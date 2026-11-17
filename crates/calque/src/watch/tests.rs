//! Tests for `watch`.
use super::*;
use crate::{compile, CompileOptions};
use tempfile::tempdir;

#[test]
fn start_delivers_events_for_a_write() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let _w = start(&[(&root, RecursiveMode::Recursive)], move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    let file = root.join("a.txt");
    std::fs::write(&file, "x").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let ev = rx.recv_timeout(left).expect("no event within 5s").unwrap();
        if ev
            .paths
            .iter()
            .any(|p| p.canonicalize().ok().as_ref() == Some(&file))
        {
            break;
        }
    }
}

/// Compile a tempdir and return its (canonical root, watch set).
fn compiled() -> (tempfile::TempDir, PathBuf, WatchSet) {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let input = root.join("proj");
    for d in ["src", "docs", "target/debug"] {
        std::fs::create_dir_all(input.join(d)).unwrap();
    }
    std::fs::write(input.join("src/a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(input.join("docs/n.md"), "# N\n\nbody\n").unwrap();
    std::fs::write(input.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(input.join("target/debug/x.rs"), "pub fn x() {}\n").unwrap();
    let r = compile(&input, &root.join("out"), &CompileOptions::default()).unwrap();
    (dir, input, r.watch_set)
}

#[test]
fn walked_source_file_is_relevant() {
    let (_d, r, w) = compiled();
    assert!(w.relevant(&r.join("src/a.rs")));
}

#[test]
fn new_handled_file_in_visited_dir_is_relevant() {
    let (_d, r, w) = compiled();
    assert!(w.relevant(&r.join("src/b.rs")));
}

#[test]
fn new_shell_script_without_an_extension_is_relevant() {
    let (_d, r, w) = compiled();
    std::fs::write(r.join("src/hook"), "#!/bin/sh\necho hi\n").unwrap();
    std::fs::write(r.join("src/NOTES"), "plain words\n").unwrap();
    assert!(w.relevant(&r.join("src/hook")));
    assert!(!w.relevant(&r.join("src/NOTES")));
}

#[test]
fn unhandled_file_is_not_relevant() {
    let (_d, r, w) = compiled();
    assert!(!w.relevant(&r.join("src/a.o")));
}

#[test]
fn ignored_tree_events_are_not_relevant() {
    let (_d, r, w) = compiled();
    std::fs::create_dir(r.join("target/debug/new_dir")).unwrap();
    assert!(!w.relevant(&r.join("target/debug/y.rs")));
    assert!(!w.relevant(&r.join("target/debug/new_dir")));
}

#[test]
fn gitignore_change_is_relevant() {
    let (_d, r, w) = compiled();
    assert!(w.relevant(&r.join(".gitignore")));
}

#[test]
fn new_visible_subdir_is_relevant() {
    let (_d, r, w) = compiled();
    std::fs::create_dir(r.join("src/sub")).unwrap();
    assert!(w.relevant(&r.join("src/sub")));
}

#[test]
fn skipped_folder_events_are_not_relevant() {
    let (_d, r, w) = compiled();
    std::fs::create_dir_all(r.join("docs/superpowers/plans")).unwrap();
    assert!(!w.relevant(&r.join("docs/superpowers")));
    assert!(!w.relevant(&r.join("docs/superpowers/plans")));
    assert!(!w.relevant(&r.join("docs/superpowers/plans/p.md")));
}

#[test]
fn hidden_new_dir_is_not_relevant() {
    let (_d, r, w) = compiled();
    std::fs::create_dir(r.join(".cache")).unwrap();
    assert!(!w.relevant(&r.join(".cache")));
}

#[test]
fn deleted_source_file_is_relevant() {
    let (_d, r, w) = compiled();
    std::fs::remove_file(r.join("src/a.rs")).unwrap();
    assert!(w.relevant(&r.join("src/a.rs")));
}

#[test]
fn repo_memory_is_relevant_before_it_exists() {
    let r = PathBuf::from("/proj");
    let w = WatchSet::new(&r, [], [r.clone()]).with_memory([r.join(".agents/memory")]);
    assert!(w.relevant(&r.join(".agents")));
    assert!(w.relevant(&r.join(".agents/memory")));
    assert!(w.relevant(&r.join(".agents/memory/new.md")));
    assert!(w.relevant(&r.join(".agents/memory/sub/deep.md")));
    assert!(!w.relevant(&r.join(".agents/memory/logs/2026-10-31.md")));
    assert!(w.relevant(&r.join(".agents/memory/sub/logs/n.md")));
    assert!(!w.relevant(&r.join(".agents/other.md")));
    assert!(!w.relevant(&r.join(".cache")));
}

#[test]
fn a_sibling_of_a_missing_memory_folder_is_not_relevant() {
    let slot = PathBuf::from("/c/projects/-proj");
    let w = WatchSet::new(Path::new("/proj"), [], [PathBuf::from("/proj")])
        .with_memory([slot.join("memory")]);
    assert!(!w.relevant(&slot.join("abc.jsonl")));
    assert!(w.relevant(&slot.join("memory")));
    assert!(w.relevant(&slot.join("memory/new.md")));
}

#[test]
fn external_memory_files_are_relevant() {
    let (d, _r, w) = compiled();
    let ext = d.path().canonicalize().unwrap().join("ext");
    let w = w.with_memory([ext.clone()]);
    assert!(w.relevant(&ext.join("n.md")));
    assert!(!w.relevant(&d.path().join("elsewhere/n.md")));
}
