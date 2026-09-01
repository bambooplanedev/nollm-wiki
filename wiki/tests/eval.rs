//! Retrieval eval harness: one comparable score for `wiki search`.
//!
//! Runs labelled queries against the frozen 17-page corpus in
//! `tests/.eval_corpus/`.
//!
//! **The corpus is frozen at commit `a55cc54` and must never be re-synced
//! with the live files it was copied from.** Re-syncing restores the moving
//! target this harness exists to remove: a score change would no longer be
//! attributable to the scorer, because the corpus could have moved instead.
//! The corpus is re-cut only by explicit decision when a scoring cycle
//! closes, in a commit that quotes both the old and the new table.
//!
//! The directory is dot-prefixed so `walk`'s `.hidden(true)` prunes it when
//! the repository root is the walk root (keeping 17 duplicate pages out of
//! the self-hosted wiki), while compiling it *as* the walk root still yields
//! every file — the walker never filters its own root.

use std::path::{Path, PathBuf};
use tempfile::TempDir;
use wiki::query::Wiki;
use wiki::{compile, CompileOptions};

/// Every page the corpus must compile to, **ascending by id** — the order
/// `Wiki::list_pages` returns, which is not the source-path order the spec's
/// table uses. An `assert_eq!` on the full list rather than a count: it also
/// catches a lost extractor, an eighteenth file, a naming-rule change that
/// moves an id, and a developer's global gitignore silently eating a corpus
/// file (the harness compiles with `CompileOptions::default()`, which honours
/// `git_global` and `git_exclude`).
const EXPECTED_IDS: &[&str] = &[
    "architecture",
    "cache",
    "cli",
    "formats",
    "generator",
    "graph_page",
    "hash",
    "lint",
    "manifest",
    "markdown",
    "model",
    "snapshot",
    "summary",
    "text",
    "walk",
    "watch",
    "wiki",
];

/// Compile the frozen corpus into a fresh tempdir and load it.
///
/// The `TempDir` is returned alongside the `Wiki` because dropping it deletes
/// the compiled pages, and `Wiki` reads page bodies from disk on demand.
fn load_corpus() -> (TempDir, Wiki) {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out");
    let corpus: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/.eval_corpus");
    compile(&corpus, &output, &CompileOptions::default()).unwrap();
    let wiki = Wiki::load(&output).unwrap();
    (dir, wiki)
}

#[test]
fn corpus_compiles_to_the_expected_pages() {
    let (_dir, wiki) = load_corpus();
    let ids: Vec<String> = wiki.list_pages().into_iter().map(|(id, _)| id).collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    assert_eq!(ids, EXPECTED_IDS, "corpus did not compile as expected");
}
