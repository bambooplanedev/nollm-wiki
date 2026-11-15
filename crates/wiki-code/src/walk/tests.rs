//! Tests for `walk`.
use super::*;
use std::fs;
use tempfile::tempdir;

/// Read every file the walk reaches: these tests exercise walk's own
/// traversal rules, not the caller's relevance filter.
fn all(_: &str) -> bool {
    true
}

#[test]
fn walk_returns_sorted_relative_files_and_honors_gitignore() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("b.txt"), b"bbb").unwrap();
    fs::write(root.join("a.txt"), b"aaa").unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/c.txt"), b"ccc").unwrap();
    fs::write(root.join(".gitignore"), b"ignored.txt\n").unwrap();
    fs::write(root.join("ignored.txt"), b"nope").unwrap();

    let files = walk(root, true, &all).files;
    let rels: Vec<_> = files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(rels, vec!["a.txt", "b.txt", "sub/c.txt"]);
    assert_eq!(files[0].bytes, b"aaa");
}

#[test]
fn walk_no_ignore_includes_gitignored() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join(".gitignore"), b"ignored.txt\n").unwrap();
    fs::write(root.join("ignored.txt"), b"nope").unwrap();
    let files = walk(root, false, &all).files;
    assert!(files.iter().any(|f| f.rel_path == "ignored.txt"));
}

#[test]
fn walk_prunes_whole_ignored_directory_subtrees() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join(".gitignore"), b"target/\n").unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    fs::write(root.join("target/debug/foo.o"), b"binary").unwrap();
    fs::write(root.join("keep.txt"), b"keep").unwrap();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main").unwrap();

    let files = walk(root, true, &all).files;
    let rels: Vec<_> = files.iter().map(|f| f.rel_path.clone()).collect();

    assert!(rels.contains(&"keep.txt".to_string()));
    assert!(
        !rels.iter().any(|r| r.starts_with("target/")),
        "target/ subtree should be pruned entirely, got: {rels:?}"
    );
    assert!(
        !rels.iter().any(|r| r.starts_with(".git/")),
        ".git/ should be excluded as a hidden directory, got: {rels:?}"
    );
}

/// Regression: a compile once held every byte of the input tree in memory,
/// including files no extractor could ever handle. An input directory that
/// happened to sit next to ~48GB of raw `.mov` footage took ~48GB of RSS
/// and never finished. `keep` has to reject on the path, before the read —
/// so the bytes are not merely dropped later, they are never allocated.
#[test]
fn unhandled_files_are_not_read_into_memory() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("notes.txt"), b"small").unwrap();
    // Stands in for the video: big enough that holding it would show up.
    fs::write(root.join("footage.mov"), vec![0u8; 8 * 1024 * 1024]).unwrap();

    let files = walk(root, true, &|p| {
        Path::new(p).extension() == Some("txt".as_ref())
    })
    .files;

    let rels: Vec<_> = files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(rels, vec!["notes.txt"]);
    let held: usize = files.iter().map(|f| f.bytes.len()).sum();
    assert_eq!(
        held, 5,
        "walk retained bytes for a file it was told to skip: {held}"
    );
}

#[test]
fn walk_memory_ignores_git_rules_but_skips_hidden_and_index_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join(".gitignore"), b"a.md\n").unwrap();
    fs::write(root.join("a.md"), b"a").unwrap();
    fs::write(root.join("MEMORY.md"), b"i").unwrap();
    fs::write(root.join("memory.md"), b"i").unwrap();
    fs::write(root.join("._a.md"), b"j").unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/b.md"), b"b").unwrap();

    let w = walk_memory(root, &all);
    let rels: Vec<_> = w.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(rels, vec!["a.md", "sub/b.md"]);
    assert_eq!(w.dirs, vec![root.to_path_buf(), root.join("sub")]);
}

/// `.git` (a directory, or a file in a linked worktree) is never counted.
fn tree_with_every_skip(root: &Path) {
    for (path, body) in [
        (".gitignore", "build/\nsecret.txt\n"),
        ("keep.txt", "k"),
        ("clip.mov", "a file `keep` refuses is reached, not skipped"),
        ("secret.txt", "s"),
        ("build/a.o", "b"),
        ("build/vendor/b.o", "outermost is the ignored build/"),
        (".hidden/c.txt", "c"),
        ("docs/superpowers/plan.md", "p"),
        ("docs/superpowers/spec.md", "p"),
        ("docs/superpowers/vendor/d.md", "outermost is superpowers"),
        ("node_modules/x/e.js", "n"),
        (".git/HEAD", "h"),
        ("sub/.git", "gitdir: elsewhere"),
        (".kulman/page.md", "the output, not a skipped source"),
        (".agents/memory/note.md", "compiled from the memory walk"),
    ] {
        let p = root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
}

#[test]
fn skipped_counts_each_file_once_for_its_outermost_rule() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    tree_with_every_skip(root);

    let own = [root.join(crate::OUT_DIR), root.join(".agents/memory")];
    assert_eq!(
        skipped(root, &own),
        vec![("superpowers", 3), ("node_modules", 1), ("ignored", 5)],
        "ignored is .gitignore, secret.txt, build/ (2) and .hidden/"
    );
}

#[test]
fn skipped_is_empty_when_nothing_is_skipped() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), b"a").unwrap();
    assert_eq!(skipped(dir.path(), &[]), []);
}

#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_read_is_listed_as_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("ok.txt"), b"ok").unwrap();
    let locked = root.join("locked.txt");
    fs::write(&locked, b"secret").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads a mode-000 file anyway: nothing to test there.
    let readable = fs::read(&locked).is_ok();

    let w = walk(root, true, &all);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        !readable,
        "needs a user that cannot read a mode 000 file (not root)"
    );
    let rels: Vec<_> = w.files.iter().map(|f| f.rel_path.clone()).collect();
    assert_eq!(rels, vec!["ok.txt"]);
    assert_eq!(w.unreadable, vec!["locked.txt"]);
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("real.txt"), b"real").unwrap();
    symlink(root.join("real.txt"), root.join("link.txt")).unwrap();

    let files = walk(root, true, &all).files;
    let rels: Vec<_> = files.iter().map(|f| f.rel_path.clone()).collect();
    assert!(
        rels.contains(&"real.txt".to_string()),
        "real file missing: {rels:?}"
    );
    assert!(
        !rels.contains(&"link.txt".to_string()),
        "symlink was followed (determinism risk): {rels:?}"
    );
}
