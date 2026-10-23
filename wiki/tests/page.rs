//! `Wiki::outline`: a page's module map, one line per code symbol, against a compiled fixture.

use std::fs;
use tempfile::tempdir;
use wiki::query::Wiki;
use wiki::{compile, CompileOptions};

const LIB_RS: &str = "//! Adds numbers.
use std::fmt;

/// Adds.
pub fn add() {}

pub struct G;
impl G {
    /// Builds.
    pub fn build(&self) {}
    fn helper(&self) {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn adds() {}
}
";

/// Compiles the fixture into `<dir>/out` and loads it.
fn fixture() -> (tempfile::TempDir, Wiki) {
    let dir = tempdir().unwrap();
    let (input, out) = (dir.path().join("raw"), dir.path().join("out"));
    let files = [
        ("src/lib.rs", LIB_RS),
        (
            "include/book.h",
            "class Book {\npublic:\n  /// Applies an order.\n  void apply(int o);\n};\n",
        ),
        (
            "src/book.cpp",
            "#include \"book.h\"\nvoid Book::apply(int o) {\n  (void)o;\n}\n",
        ),
        (
            "docs/guide.md",
            "# Guide\n\nIntro.\n\n## Setup\n\n```\n## not a heading\n```\n\n### Linux\n\n## Notes\n\nmine\n",
        ),
        ("CMakeLists.txt", "# Options\nset(X 1)\n"),
        (
            "a.py",
            "import os\n## Imports\n# - fake\ndef f():\n    pass\n",
        ),
    ];
    for (path, text) in files {
        let p = input.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    compile(&input, &out, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&out).unwrap();
    (dir, w)
}

fn outline(w: &Wiki, id: &str) -> String {
    w.outline(id, None)
        .unwrap_or_else(|| panic!("no outline for {id}"))
}

#[test]
fn rust_outline_is_pinned() {
    let (_d, w) = fixture();
    assert_eq!(
        outline(&w, "lib"),
        "# Lib (lib)
src/lib.rs  code:rust
Adds numbers.
imports:
  std::fmt
4-5  fn  pub fn add()  — Adds.
7-7  type  pub struct G
G:
  9-10  method  pub fn build(&self)  — Builds.
  11-11  method  fn helper(&self)
14-18  module [test]  mod tests
16-17  fn [test]  fn adds()
"
    );
}

#[test]
fn cpp_header_lines_show_their_twin() {
    let (_d, w) = fixture();
    let header = outline(&w, "include_book");
    assert!(
        header.contains(
            "\nBook:\n  3-4  method  void apply(int o)  → src/book.cpp:2-4  — Applies an order.\n"
        ),
        "{header}"
    );
    let source = outline(&w, "src_book");
    assert!(source.contains("  → include/book.h:3-4"), "{source}");
}

#[test]
fn body_headings_are_read_past_an_authors_notes_heading() {
    let (_d, w) = fixture();
    let guide = outline(&w, "guide");
    assert!(
        guide.ends_with("headings:\n  Guide\n    Setup\n      Linux\n    Notes\n"),
        "{guide}"
    );
    assert!(!guide.contains("not a heading"), "{guide}");
}

#[test]
fn text_page_is_header_only() {
    let (_d, w) = fixture();
    assert_eq!(
        outline(&w, "options"),
        "# Options (options)\nCMakeLists.txt  text\nset(X 1)\n"
    );
}

#[test]
fn imports_come_only_from_before_body() {
    let (_d, w) = fixture();
    let a = outline(&w, "a");
    let block: Vec<&str> = a
        .split_once("imports:\n")
        .expect("an imports block")
        .1
        .lines()
        .take_while(|l| l.starts_with("  "))
        .collect();
    assert_eq!(block, ["  os"], "{a}");
}

#[test]
fn page_without_items_json_says_no_items() {
    let (d, _) = fixture();
    let out = d.path().join("out");
    fs::remove_file(out.join("items.json")).unwrap();
    let w = Wiki::load(&out).unwrap();
    let lib = outline(&w, "lib");
    assert!(
        lib.ends_with("\nno items — recompile with a current wiki for symbol lines\n"),
        "{lib}"
    );
}

#[test]
fn unknown_id_is_none() {
    let (_d, w) = fixture();
    assert_eq!(w.outline("nope", None), None);
}
