//! Frozen-corpus helpers shared by the eval harnesses. Included with
//! `#[path = "common/corpus.rs"] mod corpus;` rather than through
//! `common/mod.rs`, so a harness that does not use them never compiles them.

use std::path::{Path, PathBuf};
use tempfile::TempDir;
use wiki::query::Wiki;
use wiki::{compile, CompileOptions};

/// Hash the corpus directory's relative paths and contents, in sorted path
/// order, so the result does not depend on directory-listing order.
///
/// Line endings are normalised (`\r\n` -> `\n`) before hashing: a corpus
/// checked out with CRLF moves no scoring metric, so it must not move this
/// hash either, or the check would fail spuriously on a Windows checkout.
pub fn corpus_hash(corpus: &Path) -> String {
    let mut files = wiki::walk::walk(corpus, false, &|_| true).unwrap().files;
    // `walk` already sorts by `rel_path`, but the hash's determinism is the
    // point here, not an incidental property of the walker, so make it
    // explicit rather than relying on that guarantee silently.
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for f in &files {
        let normalized = String::from_utf8_lossy(&f.bytes).replace("\r\n", "\n");
        parts.push(f.rel_path.clone().into_bytes());
        parts.push(normalized.into_bytes());
    }
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    wiki::hash::to_hex(&wiki::hash::combine(&refs))
}

/// Check the frozen corpus at `rel` (relative to `CARGO_MANIFEST_DIR`) against
/// `expected_hash`, compile it into a fresh tempdir and load it.
///
/// The `TempDir` is returned alongside the `Wiki` because dropping it deletes
/// the compiled pages, and `Wiki` reads page bodies from disk on demand. The
/// compiled output directory is returned for callers that read pages directly.
pub fn compile_frozen(rel: &str, expected_hash: &str) -> (TempDir, PathBuf, Wiki) {
    let corpus: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    assert_eq!(
        corpus_hash(&corpus),
        expected_hash,
        "the frozen corpus's content changed. An id-set assertion cannot \
         catch this: it only sees which pages exist, not what they say. If \
         this is a deliberate re-cut, update the harness's corpus-hash \
         constant in the same commit that quotes the old and new tables; if \
         it is not, revert the corpus.",
    );
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out");
    let opts = CompileOptions {
        respect_ignore: false,
        ..CompileOptions::default()
    };
    compile(&corpus, &output, &opts).unwrap();
    let wiki = Wiki::load(&output).unwrap();
    (dir, output, wiki)
}
