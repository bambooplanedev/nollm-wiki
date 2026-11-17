//! Tests for the crate root.

#[test]
fn stale_page_files_lists_orphaned_on_disk_only() {
    use std::collections::BTreeSet;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    std::fs::write(out.join("old_slug.md"), "x").unwrap();
    std::fs::write(out.join("kept.md"), "x").unwrap();
    let prior: BTreeSet<String> = ["old_slug", "kept", "never_written"]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    let live: BTreeSet<String> = ["kept", "new_slug"]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    // old_slug: prior, not live, on disk → listed.
    // kept: live → excluded. never_written: not on disk → excluded.
    assert_eq!(
        super::stale_page_files(&prior, &live, out),
        vec!["old_slug.md".to_string()]
    );
}

#[test]
fn is_under_matches_nested_paths_only() {
    use std::path::Path;
    assert!(super::is_under(
        Path::new("/proj/wiki/alpha.md"),
        Path::new("/proj/wiki")
    ));
    assert!(!super::is_under(
        Path::new("/proj/alpha.txt"),
        Path::new("/proj/wiki")
    ));
    // Sibling prefix must not false-match (/proj/wiki vs /proj/wiki2).
    assert!(!super::is_under(
        Path::new("/proj/wiki2/a.md"),
        Path::new("/proj/wiki")
    ));
}
