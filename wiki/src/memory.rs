//! Where local (private) agent memory lives: a per-user data dir, keyed by the repo it belongs to.

use crate::hash::{hash_str, to_hex};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Project memory kept inside the checkout.
pub const REPO_DIR: &str = ".agents/memory";

/// The per-user data dir for `os` (a `std::env::consts::OS` value), reading variables through `env`.
pub fn data_dir_for(os: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    match os {
        "windows" => var("LOCALAPPDATA").map(PathBuf::from),
        "macos" => var("HOME").map(|h| Path::new(&h).join("Library/Application Support")),
        _ => match var("XDG_DATA_HOME") {
            Some(x) => Some(PathBuf::from(x)),
            None => var("HOME").map(|h| Path::new(&h).join(".local/share")),
        },
    }
}

/// `p` as an absolute display path with `/` separators on every OS; a Windows
/// verbatim prefix (`\\?\`, `\\?\UNC\`) is dropped so page paths stay plain.
pub fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if let Some(unc) = s.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else {
        s.strip_prefix("//?/").map_or(s.clone(), str::to_string)
    }
}

/// An external memory folder as one stable absolute path: canonical when it
/// exists (no `..`, no symlink aliases, so it matches watch events), else merely absolute.
pub fn resolve(dir: &Path) -> std::io::Result<PathBuf> {
    dir.canonicalize().or_else(|_| std::path::absolute(dir))
}

pub fn data_dir() -> Option<PathBuf> {
    data_dir_for(std::env::consts::OS, &|k| std::env::var(k).ok())
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether `root` is inside a git repo.
pub fn is_git_repo(root: &Path) -> bool {
    git(root, &["rev-parse", "--git-common-dir"]).is_some()
}

/// A key shared by every checkout and worktree of one repo: the smallest root commit hash, or, when
/// history is shallow, a hash of the canonical git common dir. `None` outside a git repo and in a
/// repo with no commits yet (a key must not change at the first commit).
pub fn repo_key(root: &Path) -> Option<String> {
    let common = git(root, &["rev-parse", "--git-common-dir"])?;
    let shallow = git(root, &["rev-parse", "--is-shallow-repository"]).as_deref() == Some("true");
    git(root, &["rev-parse", "--verify", "-q", "HEAD"])?;
    if !shallow {
        if let Some(roots) = git(root, &["rev-list", "--max-parents=0", "HEAD"]) {
            if let Some(smallest) = roots.lines().filter(|l| !l.is_empty()).min() {
                return Some(smallest.to_string());
            }
        }
    }
    let dir = root.join(common);
    let dir = dir.canonicalize().unwrap_or(dir);
    Some(to_hex(&hash_str(&dir.to_string_lossy()))[..16].to_string())
}

/// The local memory folder of the repo at `root` (not created).
pub fn local_dir(root: &Path) -> Option<PathBuf> {
    Some(data_dir()?.join("wiki/memory").join(repo_key(root)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn display_path_uses_forward_slashes() {
        assert_eq!(
            display_path(Path::new(r"C:\Users\a\m\n.md")),
            "C:/Users/a/m/n.md"
        );
        assert_eq!(display_path(Path::new("/h/m/n.md")), "/h/m/n.md");
        assert_eq!(display_path(Path::new(r"\\?\C:\m\n.md")), "C:/m/n.md");
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\srv\share\n.md")),
            "//srv/share/n.md"
        );
    }

    #[test]
    fn data_dir_per_os() {
        let e = env_of(&[("LOCALAPPDATA", "C:/L"), ("HOME", "/h")]);
        assert_eq!(data_dir_for("windows", &e), Some(PathBuf::from("C:/L")));
        assert_eq!(
            data_dir_for("macos", &e),
            Some(Path::new("/h").join("Library/Application Support"))
        );
        assert_eq!(
            data_dir_for("linux", &e),
            Some(Path::new("/h").join(".local/share"))
        );
        let x = env_of(&[("XDG_DATA_HOME", "/x"), ("HOME", "/h")]);
        assert_eq!(data_dir_for("linux", &x), Some(PathBuf::from("/x")));
        let empty = env_of(&[("XDG_DATA_HOME", ""), ("HOME", "/h")]);
        assert_eq!(
            data_dir_for("linux", &empty),
            Some(Path::new("/h").join(".local/share"))
        );
        let none = env_of(&[]);
        for os in ["windows", "macos", "linux"] {
            assert_eq!(data_dir_for(os, &none), None);
        }
        assert_eq!(data_dir_for("macos", &env_of(&[("HOME", "")])), None);
    }

    fn have_git() -> bool {
        let ok = Command::new("git").arg("--version").output().is_ok();
        if !ok {
            eprintln!("git not on PATH; skipping");
        }
        ok
    }

    fn run(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    fn commit(dir: &Path, msg: &str) {
        run(dir, &["commit", "--allow-empty", "-m", msg]);
    }

    fn head(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).unwrap()
    }

    #[test]
    fn not_a_repo_has_no_key() {
        if !have_git() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        assert_eq!(repo_key(d.path()), None);
    }

    #[test]
    fn key_is_root_commit_shared_by_worktrees() {
        if !have_git() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("r");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q"]);
        commit(&repo, "one");
        let key = repo_key(&repo).unwrap();
        assert_eq!(key.len(), 40);
        assert_eq!(key, head(&repo));
        commit(&repo, "two");
        let wt = d.path().join("wt");
        run(
            &repo,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "other"],
        );
        assert_eq!(repo_key(&wt), Some(key));
    }

    #[test]
    fn no_commits_has_no_key() {
        if !have_git() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        run(d.path(), &["init", "-q"]);
        assert_eq!(repo_key(d.path()), None);
    }

    #[test]
    fn shallow_clone_does_not_use_the_boundary_commit() {
        if !have_git() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        std::fs::create_dir(&src).unwrap();
        run(&src, &["init", "-q"]);
        commit(&src, "one");
        commit(&src, "two");
        let dst = d.path().join("dst");
        let url = format!("file://{}", src.display());
        let out = Command::new("git")
            .args(["clone", "-q", "--depth", "1", &url, dst.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let key = repo_key(&dst).unwrap();
        assert_eq!(key.len(), 16);
        assert_ne!(key, head(&dst));
    }
}
