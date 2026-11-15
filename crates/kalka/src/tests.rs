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
fn prune_stale_never_deletes_hand_written_notes() {
    use std::collections::BTreeSet;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    std::fs::write(
        out.join("noted.md"),
        "# Noted\n\n## Body\nx\n\n## Notes\nwhy this module is odd\n",
    )
    .unwrap();
    std::fs::write(
        out.join("plain.md"),
        "# Plain\n\n## Body\nx\n\n## Notes\n_(add your own notes here — preserved on recompile)_\n",
    )
    .unwrap();
    std::fs::write(
        out.join("migrated.md"),
        "# Migrated\n\n## Body\nx\n\n## Notes\ncarried onto the renamed page\n",
    )
    .unwrap();
    let prunable: BTreeSet<String> = ["noted", "plain", "migrated"]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    let consumed: BTreeSet<std::path::PathBuf> = [out.join("migrated.md")].into();
    // Every id is gone from the corpus — an id-scheme change renames every
    // page at once, which is exactly when notes are easiest to lose.
    super::prune_stale(&prunable, &BTreeSet::new(), out, &consumed);
    assert!(
        out.join("noted.md").exists(),
        "the only human-owned section in the output must survive a prune"
    );
    assert!(
        !out.join("plain.md").exists(),
        "an untouched placeholder is not notes; that page is still pruned"
    );
    assert!(
        !out.join("migrated.md").exists(),
        "notes already migrated onto the renamed page leave nothing to keep"
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
