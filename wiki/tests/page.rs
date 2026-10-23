//! `Wiki::outline`: a page's module map, one line per code symbol, against a compiled fixture.

use std::fs;
use tempfile::tempdir;
use wiki::manifest::token_estimate;
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

/// `src/big.rs`: an import, 30 documented fns, 30 private fns, then a test
/// module of 30 tests. Line 2 is `f0`'s doc.
fn big_rs() -> String {
    let mut s = "use std::fmt;\n".to_string();
    for i in 0..30 {
        s += &format!("/// Doc number {i} explains it.\npub fn f{i}() {{}}\n");
    }
    for i in 0..30 {
        s += &format!("fn p{i}() {{}}\n");
    }
    s + "#[cfg(test)] mod tests {\n" + &tests_30() + "}\n"
}

fn tests_30() -> String {
    let tests: Vec<String> = (0..30).map(|i| format!("#[test] fn t{i}() {{}}")).collect();
    tests.join("\n") + "\n"
}

/// Compiles the fixture into `<dir>/out` and loads it.
fn fixture() -> (tempfile::TempDir, Wiki) {
    let dir = tempdir().unwrap();
    let (input, out) = (dir.path().join("raw"), dir.path().join("out"));
    let (big, all_tests) = (big_rs(), tests_30());
    let files = [
        ("src/big.rs", big.as_str()),
        ("tests/all_tests.rs", all_tests.as_str()),
        ("src/lib.rs", LIB_RS),
        (
            "src/wrapped.rs",
            "use tree_sitter::{\n    Language, Node,\n    Parser,\n};\npub fn w() {}\n",
        ),
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
fn a_wrapped_import_is_one_line() {
    let (_d, w) = fixture();
    let o = outline(&w, "wrapped");
    assert!(
        o.contains("\nimports:\n  tree_sitter::{ Language, Node, Parser, }\n5-5  fn"),
        "{o}"
    );
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

/// `outline(id, Some(b))` for every `b` from 40 up to the full outline's
/// tokens, in steps of 20.
fn sweep(w: &Wiki, id: &str) -> Vec<(usize, String)> {
    let full = token_estimate(&outline(w, id));
    (40..=full)
        .step_by(20)
        .map(|b| (b, w.outline(id, Some(b)).unwrap()))
        .collect()
}

/// The `…` marker line of a budgeted outline, if it has one.
fn marker(text: &str) -> Option<&str> {
    text.lines().last().filter(|l| l.starts_with("… "))
}

/// The header lines of a page with imports: every line before `imports:`.
fn header(full: &str) -> String {
    full.lines()
        .take_while(|l| *l != "imports:")
        .flat_map(|l| [l, "\n"])
        .collect()
}

#[test]
fn budget_sweep_respects_ceiling_and_cut_order() {
    let (_d, w) = fixture();
    let header = header(&outline(&w, "big"));
    for (b, text) in sweep(&w, "big") {
        let floor = marker(&text).is_some() && text.lines().count() == header.lines().count() + 1;
        assert!(token_estimate(&text) <= b || floor, "{b}: {text}");
        if let Some(m) = marker(&text) {
            assert!(!m.contains(" docs") || m.contains(" test items"), "{m}");
            assert!(!m.contains(" imports") || m.contains(" docs"), "{m}");
            assert!(
                !m.contains("more items from line") || m.contains(" imports"),
                "{m}"
            );
        }
        if text.contains(" [test] ") {
            assert_eq!(marker(&text), None, "{b}: {text}");
        }
    }
}

#[test]
fn all_test_page_never_cuts_tests() {
    let (_d, w) = fixture();
    let swept = sweep(&w, "all_tests");
    assert!(
        swept.iter().any(|(_, t)| marker(t).is_some()),
        "nothing was cut"
    );
    for (_, text) in swept {
        assert!(
            !marker(&text).unwrap_or("").contains("test items"),
            "{text}"
        );
    }
}

#[test]
fn marker_budget_returns_the_full_outline() {
    let (_d, w) = fixture();
    let full = outline(&w, "big");
    for (_, text) in sweep(&w, "big") {
        if let Some(m) = marker(&text) {
            let t = m.split("pass max_tokens: ").nth(1).unwrap();
            let t: usize = t.strip_suffix(" for all").unwrap().parse().unwrap();
            assert_eq!(w.outline("big", Some(t)).unwrap(), full);
        }
    }
}

#[test]
fn tiny_budget_keeps_header_and_marker() {
    let (_d, w) = fixture();
    let full = outline(&w, "big");
    let header = header(&full);
    assert!(header.starts_with("# Big (big)\n"), "{full}");
    let t = token_estimate(&full);
    assert_eq!(
        w.outline("big", Some(1)).unwrap(),
        format!(
            "{header}… dropped: 31 test items, 30 docs, 1 imports; 60 more items from line 2 — pass max_tokens: {t} for all\n"
        )
    );
}

#[test]
fn no_cut_no_marker() {
    let (_d, w) = fixture();
    assert_eq!(w.outline("lib", Some(4000)), w.outline("lib", None));
}

#[test]
fn markdown_ceiling_counts_headings() {
    let (_d, w) = fixture();
    let full = outline(&w, "guide");
    let (header, _) = full.split_once("headings:\n").unwrap();
    let t = token_estimate(&full);
    assert_eq!(
        w.outline("guide", Some(1)).unwrap(),
        format!("{header}… 4 more headings — pass max_tokens: {t} for all\n")
    );
}

#[test]
fn full_returns_the_rendered_page() {
    let (_d, w) = fixture();
    assert_eq!(w.page_text("lib", true, None), w.page("lib"));
    assert_eq!(w.page_text("lib", true, Some(100_000)), w.page("lib"));
    assert_eq!(w.page_text("nope", true, None), None);
}

#[test]
fn full_over_budget_code_falls_back_to_outline() {
    let (_d, w) = fixture();
    for max in [60, 61, 62, 63] {
        let t = w.page_text("lib", true, Some(max)).unwrap();
        assert!(t.starts_with("full page ~"), "{t}");
        assert!(t.contains("# Lib (lib)"), "{t}");
        assert!(token_estimate(&t) <= max, "{max}: {t}");
    }
}

#[test]
fn full_marker_budget_returns_the_whole_outline() {
    let (_d, w) = fixture();
    let first = |t: &str| t.lines().next().unwrap().to_string();
    let mut followed = 0;
    // `lib`'s page is over twice its outline, so the outline path is taken.
    for max in 20..150 {
        let t = w.page_text("lib", true, Some(max)).unwrap();
        let Some(m) = marker(&t) else { continue };
        let n = m.split("pass max_tokens: ").nth(1).unwrap();
        let n: usize = n.strip_suffix(" for all").unwrap().parse().unwrap();
        let again = w.page_text("lib", true, Some(n)).unwrap();
        assert_eq!(
            again,
            format!("{}\n{}", first(&t), outline(&w, "lib")),
            "{max} -> {n}"
        );
        assert!(token_estimate(&again) <= n, "{n}: {again}");
        followed += 1;
    }
    assert!(followed > 0, "no marker to follow");
}

#[test]
fn full_over_budget_doc_truncates_on_a_line() {
    let (_d, w) = fixture();
    let page = w.page("guide").unwrap();
    let t = w.page_text("guide", true, Some(30)).unwrap();
    let (kept, marker) = t.trim_end().rsplit_once('\n').unwrap();
    assert!(marker.starts_with("… truncated at ~"), "{t}");
    assert!(marker.ends_with(&format!(" of ~{} tokens", token_estimate(&page))));
    assert!(page.starts_with(&format!("{kept}\n")));
    assert!(token_estimate(&t) <= 30, "{t}");
}

#[test]
fn truncating_a_huge_doc_is_the_longest_whole_line_prefix() {
    let dir = tempdir().unwrap();
    let (input, out) = (dir.path().join("raw"), dir.path().join("out"));
    fs::create_dir_all(&input).unwrap();
    let lines: Vec<String> = (0..20_000).map(|i| format!("line number {i}\n")).collect();
    let body = lines.concat();
    fs::write(input.join("big.md"), format!("# Big\n\n{body}")).unwrap();
    compile(&input, &out, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&out).unwrap();
    let page = w.page("big").unwrap();
    let t = w.page_text("big", true, Some(50)).unwrap();
    let (kept, marker) = t.trim_end().rsplit_once('\n').unwrap();
    assert!(token_estimate(&t) <= 50, "{t}");
    assert!(page.starts_with(&format!("{kept}\n")));
    let b = token_estimate(&format!("{kept}\n"));
    assert_eq!(
        marker,
        format!("… truncated at ~{b} of ~{} tokens", token_estimate(&page))
    );
    // One more line would not fit.
    let more = page
        .split_inclusive('\n')
        .nth(kept.lines().count())
        .unwrap();
    let longer = format!("{kept}\n{more}");
    let m = format!(
        "… truncated at ~{} of ~99999 tokens\n",
        token_estimate(&longer)
    );
    assert!(token_estimate(&(longer + &m)) > 50);
}
