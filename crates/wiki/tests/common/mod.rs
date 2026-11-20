// Shared by the integration tests that need to make assertions about a
// specific rendered section of a page (`## Exports`, `## Imports`, ...)
// rather than the whole page. `## Body` always carries the untouched raw
// source, so a whole-page `contains` check is satisfied by source text
// regardless of what the extractor actually captured — it pins nothing. Any
// assertion about extraction correctness must scope to the curated section.
//
// `tests/common/mod.rs` (not `tests/common.rs`) is the standard idiom for
// code shared between integration-test binaries: Cargo only treats files
// directly under `tests/` as test crates, so this file is not compiled as
// its own test.

#![allow(dead_code)]

/// The tail of a `callers` line that found no caller, up to the symbol name.
pub const CLAUSE: &str = " (not seen: a call outside any function, a function passed as a value or called by a computed name, or a call from another language or an unindexed file; grep for ";

use std::path::{Path, PathBuf};
use tempfile::TempDir;
use wiki::query::Wiki;
use wiki::{compile, CompileOptions};

/// Lines under `## {name}` up to the next `## ` header, with the leading
/// `- ` list marker stripped. Panics if the section is absent, so a broken
/// extractor that drops a section entirely fails loudly instead of letting
/// a negative assertion (`!v.contains(...)`) pass vacuously over an empty
/// `Vec`.
pub fn section(page: &str, name: &str) -> Vec<String> {
    let header = format!("## {name}");
    let mut lines = page.lines();
    let found = lines.by_ref().any(|l| l == header);
    assert!(found, "page has no `{header}` section:\n{page}");
    lines
        .take_while(|l| !l.starts_with("## "))
        .filter_map(|l| l.strip_prefix("- ").map(std::string::ToString::to_string))
        .collect()
}

/// `## Exports` entries, with the surrounding backticks stripped.
pub fn exports(page: &str) -> Vec<String> {
    section(page, "Exports")
        .into_iter()
        .map(|l| l.trim_start_matches('`').trim_end_matches('`').to_string())
        .collect()
}

/// `## Imports` entries (rendered without backticks already).
pub fn imports(page: &str) -> Vec<String> {
    section(page, "Imports")
}

/// Write `body` to `root/name`, creating any missing parent directories, so a
/// fixture can lay out a nested source tree in one call per file.
pub fn write(root: &Path, name: &str, body: &str) {
    let p = root.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

/// Write `files` (relative path, contents) under `<tmp>/raw` and compile them
/// with default options into `<tmp>/out`. Returns `(tmp, raw, out)`; keep the
/// `TempDir` alive for as long as the output is read.
pub fn compiled(files: &[(&str, &str)]) -> (TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let (raw, out) = (dir.path().join("raw"), dir.path().join("out"));
    std::fs::create_dir_all(&raw).unwrap();
    for (name, body) in files {
        write(&raw, name, body);
    }
    compile(&raw, &out, &CompileOptions::default()).unwrap();
    (dir, raw, out)
}

/// `compiled`, then load the output as a `Wiki`.
pub fn compiled_wiki(files: &[(&str, &str)]) -> (TempDir, Wiki) {
    let (dir, _, out) = compiled(files);
    let wiki = Wiki::load(&out).unwrap();
    (dir, wiki)
}

/// The seeded (42) generated corpus of `n` files, compiled into `<tmp>/out`.
pub fn seed_corpus(n: usize) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let (raw, out) = (dir.path().join("raw"), dir.path().join("out"));
    wiki::generator::generate_corpus(&raw, n, 42).unwrap();
    compile(&raw, &out, &CompileOptions::default()).unwrap();
    (dir, out)
}

/// Parsed `index.json` of a compiled output directory.
pub fn index(out: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(out.join("index.json")).unwrap()).unwrap()
}

/// The `entries` array of `index.json`.
pub fn index_entries(out: &Path) -> Vec<serde_json::Value> {
    index(out)["entries"].as_array().unwrap().clone()
}

/// The `index.json` entry with the given id. Panics if absent.
pub fn entry(out: &Path, id: &str) -> serde_json::Value {
    index_entries(out)
        .into_iter()
        .find(|e| e["id"] == id)
        .unwrap_or_else(|| panic!("no index entry `{id}`"))
}

/// Put an executable file at `path`, written by a child process. Written from
/// the test process, its write fd leaks into any child another test thread
/// forks meanwhile and stays open there until that child execs, and running
/// `path` in that window fails with ETXTBSY ("Text file busy") on Linux.
pub fn write_exe(path: &Path, content: &[u8]) {
    use std::io::Write;
    use std::process::Stdio;
    let mut sh = std::process::Command::new("sh")
        .args(["-c", "cat > \"$0\" && chmod 755 \"$0\""])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    sh.stdin.take().unwrap().write_all(content).unwrap();
    assert!(sh.wait().unwrap().success(), "writing {}", path.display());
}

/// A copy of the wiki binary at `path`, through [`write_exe`].
pub fn copy_wiki(path: &Path) {
    write_exe(path, &std::fs::read(env!("CARGO_BIN_EXE_wiki")).unwrap());
}
