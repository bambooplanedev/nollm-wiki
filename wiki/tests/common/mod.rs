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

/// A memory note file with `title`, `extra` front matter lines and `body`.
pub fn note(title: &str, extra: &str, body: &str) -> String {
    format!("---\ntitle: {title}\n{extra}---\n\n{body}\n")
}

/// A distinct email per author name.
pub fn email_of(name: &str) -> String {
    format!("{}@t", name.replace(' ', "."))
}

/// Run git in `dir` as author `name` (with `email_of(name)`), unsigned.
pub fn git_as(dir: &Path, name: &str, args: &[&str]) {
    let s = std::process::Command::new("git")
        .args(["-c", &format!("user.name={name}")])
        .args(["-c", &format!("user.email={}", email_of(name))])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(s.status.success(), "git {args:?}");
}

/// A temp git repo: `committed` `.agents/memory/<name>` notes committed by
/// `author` (the repo's own `user.email` is Jane Doe's), then `local`
/// agent-folder notes. Compiled into `<tmp>/out`; returns `(tmp, out)`.
pub fn origin_compiled(
    author: &str,
    committed: &[(&str, String)],
    local: &[(&str, String)],
) -> (TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write(root, "proj/a.rs", "pub fn a() {}\n");
    for (n, t) in committed {
        write(root, &format!("proj/.agents/memory/{n}.md"), t);
    }
    git_as(root, "t", &["init", "-q"]);
    git_as(root, "t", &["config", "user.email", &email_of("Jane Doe")]);
    git_as(root, author, &["add", "."]);
    git_as(root, author, &["commit", "-qm", "x"]);
    for (n, t) in local {
        write(root, &format!("agent/{n}.md"), t);
    }
    let opts = CompileOptions {
        memory: vec![root.join("agent")],
        ..Default::default()
    };
    let out = root.join("out");
    compile(&root.join("proj"), &out, &opts).unwrap();
    (d, out)
}

/// Memory notes as (file stem, text).
pub type Notes = Vec<(&'static str, String)>;

/// A co-author's committed note that says the deploy key moved, and the
/// person's local note that says where it lives: `(committed, local)`.
pub fn conflict_notes() -> (Notes, Notes) {
    (
        vec![(
            "rotation",
            note("Deploy key rotation", "", "deploy key now lives in vault-b"),
        )],
        vec![(
            "deploy",
            note("Deploy key", "", "deploy key lives in vault-a"),
        )],
    )
}

/// `conflict_notes` committed by Mallory Example, compiled into `<tmp>/out`.
pub fn conflict_compiled() -> (TempDir, PathBuf) {
    let (committed, local) = conflict_notes();
    origin_compiled("Mallory Example", &committed, &local)
}
