//! Tests for the crate root.
use super::*;
use std::collections::HashMap;

/// A distinct email per author name.
fn email_of(name: &str) -> String {
    format!("{}@t", name.replace(' ', "."))
}

fn git_in(dir: &Path, name: &str, args: &[&str]) {
    let s = Command::new("git")
        .args(["-c", &format!("user.name={name}")])
        .args(["-c", &format!("user.email={}", email_of(name))])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(s.status.success(), "git {args:?}");
}

#[test]
fn repo_origins_reads_committed_edited_staged_untracked() {
    let d = tempfile::tempdir().unwrap();
    let (root, mem) = (d.path(), d.path().join("m"));
    std::fs::create_dir(&mem).unwrap();
    git_in(root, "t", &["init", "-q"]);
    // An email no author has, so the global `user.email` cannot leak in.
    git_in(root, "t", &["config", "user.email", "nobody@t"]);
    let commit = |file: &str, who: &str| {
        std::fs::write(mem.join(file), file).unwrap();
        git_in(root, who, &["add", "."]);
        git_in(root, who, &["commit", "-qm", file]);
    };
    commit("a.md", "Олена Ш.");
    commit("b.md", "Jane Doe");
    std::fs::write(mem.join("b.md"), "changed").unwrap();
    std::fs::write(mem.join("c.md"), "c").unwrap();
    git_in(root, "t", &["add", "m/c.md"]);
    std::fs::write(mem.join("d.md"), "d").unwrap();
    let repo = |author: &str, edited| Origin::Repo {
        author: Some(author.to_string()),
        edited,
        mine: false,
    };
    assert_eq!(
        repo_origins(&mem),
        BTreeMap::from([
            ("a.md".to_string(), repo("Олена Ш.", false)),
            ("b.md".to_string(), repo("Jane Doe", true)),
        ])
    );
}

#[test]
fn repo_origins_ignores_show_signature_config() {
    let d = tempfile::tempdir().unwrap();
    let (root, mem) = (d.path(), d.path().join("m"));
    std::fs::create_dir(&mem).unwrap();
    git_in(root, "t", &["init", "-q"]);
    git_in(root, "t", &["config", "log.showSignature", "true"]);
    git_in(root, "t", &["config", "user.email", "nobody@t"]);
    std::fs::write(mem.join("a.md"), "a").unwrap();
    git_in(root, "Jane Doe", &["add", "."]);
    git_in(root, "Jane Doe", &["commit", "-qm", "a"]);
    assert_eq!(
        repo_origins(&mem)["a.md"],
        Origin::Repo {
            author: Some("Jane Doe".to_string()),
            edited: false,
            mine: false
        }
    );
}

#[test]
fn repo_origins_marks_own_notes() {
    let d = tempfile::tempdir().unwrap();
    let (root, mem) = (d.path(), d.path().join("m"));
    std::fs::create_dir(&mem).unwrap();
    git_in(root, "t", &["init", "-q"]);
    git_in(root, "t", &["config", "user.email", "JANE.Doe@t"]);
    for (file, who) in [("a.md", "Jane Doe"), ("b.md", "Mallory Example")] {
        std::fs::write(mem.join(file), file).unwrap();
        git_in(root, who, &["add", "."]);
        git_in(root, who, &["commit", "-qm", file]);
    }
    let mine = |n: &str| matches!(repo_origins(&mem)[n], Origin::Repo { mine, .. } if mine);
    assert!(mine("a.md"));
    assert!(!mine("b.md"));
}

#[test]
fn repo_origins_tells_namesakes_apart_by_email() {
    let d = tempfile::tempdir().unwrap();
    let (root, mem) = (d.path(), d.path().join("m"));
    std::fs::create_dir(&mem).unwrap();
    git_in(root, "t", &["init", "-q"]);
    git_in(root, "t", &["config", "user.email", "Jane.Doe@t"]);
    for (file, email) in [("a.md", "Jane.Doe@t"), ("b.md", "other.jane@t")] {
        std::fs::write(mem.join(file), file).unwrap();
        git_in(root, "Jane Doe", &["add", "."]);
        let email = format!("user.email={email}");
        git_in(root, "Jane Doe", &["-c", &email, "commit", "-qm", file]);
    }
    let origins = repo_origins(&mem);
    let name =
        |n: &str| matches!(&origins[n], Origin::Repo { author: Some(a), .. } if a == "Jane Doe");
    let mine = |n: &str| matches!(origins[n], Origin::Repo { mine, .. } if mine);
    assert!(name("a.md") && name("b.md"));
    assert!(mine("a.md"));
    assert!(!mine("b.md"));
}

#[test]
fn repo_origins_without_head_is_empty() {
    let d = tempfile::tempdir().unwrap();
    git_in(d.path(), "t", &["init", "-q"]);
    std::fs::write(d.path().join("x.md"), "x").unwrap();
    assert!(repo_origins(d.path()).is_empty());
}

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

#[test]
fn claude_slug_matches_the_cli() {
    assert_eq!(
        claude_slug("/srv/shared/my-app(v2)"),
        "-srv-shared-my-app-v2-"
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
