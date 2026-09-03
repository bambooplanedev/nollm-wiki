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
pub fn write(root: &std::path::Path, name: &str, body: &str) {
    let p = root.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}
