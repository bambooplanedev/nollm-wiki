//! Where agent memory lives: the repo's own folder, and the folders agents keep outside it.

use crate::hash::{hash_str, to_hex};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Project memory kept inside the checkout.
pub const REPO_DIR: &str = ".agents/memory";

/// Claude Code's project folder name for `path`: every UTF-16 code unit that
/// is not an ASCII letter or digit becomes `-`; past 200 units the name is cut
/// and suffixed with a hash of the whole path, as the CLI does.
pub fn claude_slug(path: &str) -> String {
    let slug: Vec<u16> = path
        .encode_utf16()
        .map(|u| match u8::try_from(u) {
            Ok(b) if b.is_ascii_alphanumeric() => u,
            _ => u16::from(b'-'),
        })
        .collect();
    let mut out = String::from_utf16_lossy(&slug[..slug.len().min(200)]);
    if slug.len() > 200 {
        let mut n = js_hash(path).unsigned_abs();
        let mut digits = Vec::new();
        while n > 0 {
            digits.push(char::from_digit(n % 36, 36).unwrap_or('0'));
            n /= 36;
        }
        if digits.is_empty() {
            digits.push('0');
        }
        out.push('-');
        out.extend(digits.iter().rev());
    }
    out
}

/// The Java-style string hash over UTF-16 code units, wrapping to `i32`.
fn js_hash(path: &str) -> i32 {
    path.encode_utf16().fold(0i32, |h, u| {
        h.wrapping_shl(5).wrapping_sub(h).wrapping_add(i32::from(u))
    })
}

/// Claude Code's memory folder for the project at `root` (it may not exist):
/// `<config>/projects/<slug>/memory`, keyed on the main checkout of the repo
/// (so a worktree or a subdirectory shares it), else on `root` itself.
/// `None` when neither `CLAUDE_CONFIG_DIR` nor a home dir is set.
pub fn claude_memory_dir(root: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    let config = match var("CLAUDE_CONFIG_DIR") {
        Some(c) => PathBuf::from(c),
        None => Path::new(&var("HOME").or_else(|| var("USERPROFILE"))?).join(".claude"),
    };
    let key = git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(PathBuf::from)
    .filter(|c| c.file_name().is_some_and(|n| n == ".git"))
    .and_then(|c| c.parent().map(Path::to_path_buf))
    .unwrap_or_else(|| root.to_path_buf());
    let key = key
        .canonicalize()
        .or_else(|_| std::path::absolute(&key))
        .unwrap_or(key);
    Some(
        config
            .join("projects")
            .join(claude_slug(&display_path(&key)))
            .join("memory"),
    )
}

/// The memory folders agents keep for the project at `root`, in a fixed
/// order, each resolved as [`resolve`] does; they may not exist yet.
pub fn agent_dirs(root: &Path) -> Vec<PathBuf> {
    [claude_memory_dir(root, &|k| std::env::var(k).ok())]
        .into_iter()
        .flatten()
        .map(|d| resolve(&d).unwrap_or(d))
        .collect()
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
/// exists (no `..`, no symlink aliases, so it matches watch events), else its
/// canonical parent plus its name, else merely absolute.
pub fn resolve(dir: &Path) -> std::io::Result<PathBuf> {
    dir.canonicalize().or_else(|_| {
        let abs = std::path::absolute(dir)?;
        Ok(
            match (abs.parent().map(Path::canonicalize), abs.file_name()) {
                (Some(Ok(p)), Some(name)) => p.join(name),
                _ => abs,
            },
        )
    })
}

pub(crate) fn git(root: &Path, args: &[&str]) -> Option<String> {
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

// removal path only; delete one release after 0.6

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

pub fn data_dir() -> Option<PathBuf> {
    data_dir_for(std::env::consts::OS, &|k| std::env::var(k).ok())
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

    #[cfg(unix)]
    #[test]
    fn resolve_a_missing_folder_through_its_canonical_parent() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().canonicalize().unwrap().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, d.path().join("link")).unwrap();
        assert_eq!(
            resolve(&d.path().join("link/mem")).unwrap(),
            real.join("mem")
        );
    }

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

    #[test]
    fn claude_slug_matches_the_cli() {
        assert_eq!(
            claude_slug("/Users/Shared/llm-wiki(nollm)"),
            "-Users-Shared-llm-wiki-nollm-"
        );
        assert_eq!(claude_slug("/home/u/проєкт"), "-home-u-------");
        assert_eq!(claude_slug("/srv/é"), "-srv--");
        assert_eq!(claude_slug(r"C:\Users\a\repo"), "C--Users-a-repo");
        let long = format!("/p/{}/{}/x", "a1".repeat(60), "b".repeat(85));
        assert_eq!(long.len(), 211);
        assert_eq!(js_hash(&long), -1_792_152_342);
        let head = format!("-p-{}-{}", "a1".repeat(60), "b".repeat(76));
        assert_eq!(head.len(), 200);
        assert_eq!(claude_slug(&long), format!("{head}-tn01li"));
    }

    #[test]
    fn claude_memory_dir_uses_config_dir_then_home() {
        let d = tempfile::tempdir().unwrap();
        let key = d.path().canonicalize().unwrap();
        let slug = claude_slug(&display_path(&key));
        let at = |base: &str| Some(Path::new(base).join("projects").join(&slug).join("memory"));
        let dir = |pairs: &[(&str, &str)]| claude_memory_dir(d.path(), &env_of(pairs));
        assert_eq!(
            dir(&[("CLAUDE_CONFIG_DIR", "/c"), ("HOME", "/h")]),
            at("/c")
        );
        assert_eq!(dir(&[("HOME", "/h")]), at("/h/.claude"));
        assert_eq!(
            dir(&[("CLAUDE_CONFIG_DIR", ""), ("HOME", "/h")]),
            at("/h/.claude")
        );
        assert_eq!(dir(&[]), None);
    }

    #[test]
    fn claude_memory_dir_keys_on_the_main_worktree() {
        if !have_git() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let env = env_of(&[("CLAUDE_CONFIG_DIR", "/c")]);
        let repo = d.path().join("r");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        run(&repo, &["init", "-q"]);
        commit(&repo, "one");
        let wt = d.path().join("wt");
        run(
            &repo,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "other"],
        );
        let main = claude_memory_dir(&repo, &env).unwrap();
        let slug = claude_slug(&display_path(&repo.canonicalize().unwrap()));
        assert_eq!(main, Path::new("/c/projects").join(slug).join("memory"));
        assert_eq!(claude_memory_dir(&wt, &env).unwrap(), main);
        assert_eq!(claude_memory_dir(&repo.join("sub"), &env).unwrap(), main);
        let plain = d.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        let slug = claude_slug(&display_path(&plain.canonicalize().unwrap()));
        assert_eq!(
            claude_memory_dir(&plain, &env).unwrap(),
            Path::new("/c/projects").join(slug).join("memory")
        );
    }
}
