//! `kind: "tests"` and `kind: "code"` without test pages: the split, its
//! scores, the pointer line and the names-only answer.

use assert_cmd::Command;
use wiki::model::KindFilter;
use wiki::query::{Hit, Wiki};

mod common;

/// Code pages only, so a search with no kind ranks the same pages a `code`
/// search did before the split. "ledger" is in one source page and three
/// test pages: statistics over source pages alone would give it another idf.
const FILES: &[(&str, &str)] = &[
    (
        "src/ledger.rs",
        "/// Posts a ledger balance.\npub fn post_balance() {}\n",
    ),
    ("src/other.rs", "pub fn unrelated() {}\n"),
    (
        "tests/ledger_flow.rs",
        "fn ledger_balance_round_trip() { post_balance(); }\nfn only_in_tests() {}\n",
    ),
    ("tests/ledger_more.rs", "fn ledger_balance_twice() {}\n"),
    ("src/ledger_test.rs", "fn ledger_balance_case() {}\n"),
];

fn fixture() -> (tempfile::TempDir, Wiki) {
    common::compiled_wiki(FILES)
}

fn is_test(h: &Hit) -> bool {
    wiki::model::is_test_path(&h.path)
}

fn paths(hits: &[Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.path.as_str()).collect()
}

#[test]
fn code_leaves_out_test_pages_and_tests_keeps_only_them() {
    let (_d, w) = fixture();
    let q = "ledger balance";
    let code = w.search(q, Some(&KindFilter::Code), 10);
    assert!(
        paths(&code).contains(&"src/ledger.rs"),
        "{:?}",
        paths(&code)
    );
    assert!(!code.iter().any(is_test), "{:?}", paths(&code));
    let tests = w.search(q, Some(&KindFilter::Tests), 10);
    assert_eq!(tests.len(), 3, "{:?}", paths(&tests));
    assert!(tests.iter().all(is_test), "{:?}", paths(&tests));
    let all = w.search(q, None, 10);
    assert_eq!(all.len(), 4, "{:?}", paths(&all));
    // `code:<lang>` follows `code`.
    let rust = w.search(q, KindFilter::parse("code:rust").as_ref(), 10);
    assert_eq!(paths(&rust), ["src/ledger.rs"]);
    // The names-only lift follows the split.
    let names = w.search("ledger_balance_twice", Some(&KindFilter::Code), 10);
    assert!(!names.iter().any(is_test), "{:?}", paths(&names));
}

#[test]
fn the_split_keeps_every_score() {
    let (_d, w) = fixture();
    for q in ["ledger balance", "ledger", "post balance round trip"] {
        let all = w.search(q, None, 10);
        let score = |id: &str| all.iter().find(|h| h.id == id).map(|h| h.score);
        for kind in [KindFilter::Code, KindFilter::Tests] {
            for h in w.search(q, Some(&kind), 10) {
                assert_eq!(Some(h.score), score(&h.id), "{q:?} {kind:?} {}", h.path);
            }
        }
    }
}

#[test]
fn tests_is_a_kind_argument() {
    assert_eq!(KindFilter::parse("tests"), Some(KindFilter::Tests));
    assert!(KindFilter::EXPECTED.contains("tests"));
}

#[test]
fn code_says_when_a_test_page_would_come_first() {
    let (_d, w) = fixture();
    // Only when a test page would have been the first page.
    let mut seen = (false, false);
    for q in [
        "ledger balance",
        "post balance",
        "unrelated",
        "ledger",
        "round trip",
    ] {
        for limit in 1..=5 {
            let unsplit = w.search(q, None, limit);
            let (hits, cut) = w.search_answer(q, Some(&KindFilter::Code), None, Some(limit));
            assert_eq!(
                cut,
                unsplit.first().is_some_and(is_test),
                "{q:?} limit {limit}"
            );
            assert!(!hits.iter().any(is_test));
            if !cut && unsplit.iter().any(is_test) {
                seen.0 = true;
            }
            seen.1 |= cut;
        }
    }
    assert_eq!(seen, (true, true), "both cases occur");
    // Only `code` says it.
    for kind in [None, Some(KindFilter::Tests)] {
        let (_, cut) = w.search_answer("ledger balance", kind.as_ref(), None, Some(5));
        assert!(!cut, "{kind:?}");
    }
}

#[test]
fn a_name_only_tests_define_is_not_missing_but_in_tests() {
    let (_d, w) = fixture();
    let code = Some(&KindFilter::Code);
    for q in ["only_in_tests", "ledger_flow"] {
        let missing = w.not_found(q, code, None);
        assert_eq!(missing, [q]);
        assert_eq!(w.in_other_kind(&missing, code, None), [q]);
        assert_eq!(w.not_found(q, None, None), Vec::<String>::new(), "{q}");
        assert_eq!(
            w.not_found(q, Some(&KindFilter::Tests), None),
            Vec::<String>::new()
        );
    }
    // A name nothing defines stays plain missing.
    let missing = w.not_found("no_such_name", code, None);
    assert_eq!(missing, ["no_such_name"]);
    assert_eq!(w.in_other_kind(&missing, code, None), Vec::<String>::new());
    // Only a `code` search points at tests.
    let named = ["only_in_tests".to_string()];
    assert_eq!(w.in_other_kind(&named, None, None), Vec::<String>::new());
}

fn text(w: &Wiki, q: &str, kind: Option<&KindFilter>) -> String {
    wiki::serve::search_text(w, q, kind, None, Some(10))
}

#[test]
fn the_missing_line_says_what_the_pages_below_are() {
    let (_d, w) = fixture();
    // Ranked pages follow: they match the name's words, not the name.
    let t = text(&w, "ledger_balance", None);
    assert!(
        t.starts_with("no symbol named ledger_balance; the pages below match its words\n"),
        "{t}"
    );
    let t = text(&w, "ledger_balance balance_ledger", None);
    assert!(
        t.starts_with(
            "no symbol named ledger_balance, balance_ledger; the pages below match their words\n"
        ),
        "{t}"
    );
    // Only another name's definition follows: no claim about it.
    let t = text(&w, "ledger_nowhere post_balance", None);
    assert!(t.starts_with("no symbol named ledger_nowhere\n"), "{t}");
    assert!(t.contains("src/ledger.rs:1-2  post_balance"), "{t}");
    // Nothing follows.
    assert_eq!(text(&w, "zq_xv", None), "no symbol named zq_xv\n");
}

#[test]
fn the_answer_lines_name_the_tests_kind() {
    let (_d, w) = fixture();
    let code = Some(&KindFilter::Code);
    let pointer = "tests also match";
    // The test definition itself, and no ranked pages for the name.
    let t = text(&w, "only_in_tests", code);
    let head = "only_in_tests: in test pages only; use kind \"tests\"\n";
    assert!(t.starts_with(head), "{t}");
    assert!(
        t.contains("\n  tests/ledger_flow.rs:2-2  only_in_tests"),
        "{t}"
    );
    assert!(!t.contains("src/") && !t.contains(pointer), "{t}");
    assert_eq!(t.lines().count(), 3, "{t}");
    // A test file's module name: its page line.
    let t = text(&w, "ledger_flow", code);
    assert!(t.contains("  tests/ledger_flow.rs  "), "{t}");
    assert!(!t.contains("src/"), "{t}");
    // The other names keep their answer, ranked by their own words.
    let t = text(&w, "ledger_balance only_in_tests", code);
    assert!(t.starts_with(head), "{t}");
    assert!(
        t.contains("no symbol named ledger_balance; the pages below match its words\n"),
        "{t}"
    );
    assert!(t.contains("  src/ledger.rs  "), "{t}");
    let t = text(&w, "post_balance only_in_tests", code);
    assert!(t.starts_with(head), "{t}");
    assert!(t.contains("src/ledger.rs:1-2  post_balance"), "{t}");
    assert!(!t.contains("no symbol named"), "{t}");
    assert_eq!(text(&w, "zq", code), "no matches\n");
}

/// The mirror of the line above: under `tests`, a name only code defines is
/// not missing; the answer says where code defines it.
#[test]
fn a_name_only_code_defines_is_not_missing_under_tests() {
    let (_d, w) = fixture();
    let tests = Some(&KindFilter::Tests);
    let missing = w.not_found("post_balance", tests, None);
    assert_eq!(missing, ["post_balance"]);
    assert_eq!(w.in_other_kind(&missing, tests, None), ["post_balance"]);
    let t = text(&w, "post_balance", tests);
    assert!(
        t.starts_with("post_balance: in code pages only; use kind \"code\"\n"),
        "{t}"
    );
    assert!(t.contains("\n  src/ledger.rs:1-2  post_balance"), "{t}");
    assert!(!t.contains("no symbol named"), "{t}");
    // The test pages that mention it follow, as `tests` ranked them on main.
    let (pointer, ranked) = t
        .split_once("\nthe test pages below match its words\n")
        .expect(&t);
    assert!(!pointer.contains("tests/"), "{t}");
    assert!(ranked.contains("  tests/ledger_flow.rs  "), "{t}");
    assert!(!ranked.contains("src/"), "{t}");
    // Another name keeps its own answer.
    let t = text(&w, "post_balance only_in_tests", tests);
    assert!(t.starts_with("post_balance: in code pages only"), "{t}");
    assert!(t.contains("tests/ledger_flow.rs:2-2  only_in_tests"), "{t}");
    let t = text(&w, "post_balance zq_xv", tests);
    assert!(
        t.contains("no symbol named zq_xv; the pages below match their words\n"),
        "{t}"
    );
    let v = json(&w, "post_balance", tests);
    assert_eq!(v[0]["code_only"], serde_json::json!(["post_balance"]));
    assert_eq!(v[0]["path"], "src/ledger.rs");
    assert_eq!(v[1]["path"], "tests/ledger_flow.rs");
    // The first ranked test page is marked, as the line answer marks it.
    assert_eq!(v[1]["pages_match_words"], true);
    assert!(v[0].get("pages_match_words").is_none(), "{}", v[0]);
    assert!(v[0].get("tests_only").is_none(), "{}", v[0]);
}

fn json(w: &Wiki, q: &str, kind: Option<&KindFilter>) -> Vec<serde_json::Value> {
    let text = wiki::serve::search_json_text(w, q, kind, None, Some(10));
    serde_json::from_str(&text).unwrap()
}

/// The JSON answer carries what the line answer says, on its first hit,
/// and lists the same pages in the same order.
#[test]
fn the_json_answer_carries_the_line_signals() {
    let (_d, w) = fixture();
    let code = Some(&KindFilter::Code);
    let first = |q: &str, kind| json(&w, q, kind).into_iter().next().unwrap();
    let names = |v: &serde_json::Value| v.as_array().unwrap().clone();
    let t = first("only_in_tests", code);
    assert_eq!(names(&t["tests_only"]), ["only_in_tests"]);
    assert_eq!(t["path"], "tests/ledger_flow.rs");
    let t = first("ledger_balance", None);
    assert_eq!(names(&t["no_symbol"]), ["ledger_balance"]);
    assert_eq!(first("ledger round trip", code)["tests_also_match"], true);
    // Nothing to say: no new field.
    let t = first("ledger balance", code);
    for k in ["no_symbol", "tests_only", "tests_also_match"] {
        assert!(t.get(k).is_none(), "{k}: {t}");
    }
    for (q, kind) in [
        ("only_in_tests", code),
        ("ledger_balance only_in_tests", code),
        ("ledger_balance", None),
        ("ledger round trip", code),
        ("post_balance", None),
    ] {
        let lines = text(&w, q, kind);
        let from_lines: Vec<&str> = lines
            .lines()
            .filter(|l| !l.starts_with(' ') && l.split("  ").count() > 2)
            .map(|l| l.split("  ").nth(1).unwrap())
            .collect();
        let from_json: Vec<String> = json(&w, q, kind)
            .iter()
            .map(|h| h["path"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(from_json, from_lines, "{q}");
    }
}

fn cli(out: &std::path::Path, args: &[&str]) -> String {
    let o = Command::cargo_bin("wiki")
        .unwrap()
        .arg("search")
        .args(args)
        .arg("--dir")
        .arg(out)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}

#[test]
fn cli_search_splits_tests() {
    let (_d, _, out) = common::compiled(FILES);
    let pointer = "tests also match; use kind \"tests\"\n";
    let code = cli(&out, &["round trip", "--kind", "code"]);
    assert!(code.ends_with(pointer), "{code}");
    let code = cli(&out, &["ledger balance", "--kind", "code"]);
    assert!(
        !code.contains(pointer) && !code.contains("tests/"),
        "{code}"
    );
    let tests = cli(&out, &["ledger balance", "--kind", "tests"]);
    assert!(tests.contains("tests/ledger_flow.rs"), "{tests}");
    assert!(
        !tests.contains("src/ledger.rs") && !tests.contains(pointer),
        "{tests}"
    );
    let all = cli(&out, &["ledger balance"]);
    assert!(all.contains("tests/") && !all.contains(pointer), "{all}");
    let names = cli(&out, &["only_in_tests", "--kind", "code"]);
    assert!(
        names.starts_with("only_in_tests: in test pages only; use kind \"tests\"\n"),
        "{names}"
    );
    assert!(!names.contains(pointer), "{names}");
    let names = cli(&out, &["only_in_tests", "--kind", "tests"]);
    assert!(
        names.contains("tests/ledger_flow.rs:2-2  only_in_tests"),
        "{names}"
    );
}

/// `page`, `callers` and the prompt hint see test pages as before.
#[test]
fn page_callers_and_hint_keep_test_pages() {
    let (_d, w) = fixture();
    let id = w.resolve("tests/ledger_flow.rs").unwrap();
    let page = w.page_text(&id, false, None).unwrap();
    assert!(page.contains("only_in_tests"), "{page}");
    let src = w.resolve("src/ledger.rs").unwrap();
    let callers = w.callers(&src, "post_balance", None, None).unwrap();
    assert!(callers.contains("ledger_balance_round_trip"), "{callers}");
    let hint = wiki::hint::hint_lines(&w, "fix the ledger balance round trip twice case");
    assert!(hint.contains("tests/ledger_"), "{hint}");
}

/// A header past the member cap: its members are no items, and a query for
/// one says so instead of "no symbol named".
#[test]
fn a_member_of_a_file_past_the_cap_is_not_called_missing() {
    let cap = wiki_code::MEMBERS_PER_FILE;
    let mut header = String::new();
    for r in 0..=cap / 10 {
        header += &format!("struct Block{r} {{\n");
        for b in 0..10 {
            header += &format!("  unsigned slot{r}_{b} : 1;\n");
        }
        header += "};\n";
    }
    let (_d, w) = common::compiled_wiki(&[
        ("src/blocks.h", header.as_str()),
        ("src/pane.h", "struct Pane {\n  int pane_width;\n};\n"),
    ]);
    let n = cap + 10;
    let t = text(&w, "slot3_4", None);
    assert!(
        t.starts_with(&format!(
            "slot3_4: members of src/blocks.h are not indexed ({n} there, more than {cap})\n"
        )),
        "{t}"
    );
    assert!(!t.contains("no symbol named"), "{t}");
    let v = json(&w, "slot3_4", None);
    assert_eq!(
        v[0]["unindexed"],
        serde_json::json!([{"name": "slot3_4", "path": "src/blocks.h"}])
    );
    // A name the file does not hold is still missing; other files index theirs.
    for q in ["slot_nowhere", "lot3_4", "slot3_"] {
        let t = text(&w, q, None);
        assert!(t.starts_with(&format!("no symbol named {q}")), "{t}");
    }
    assert!(text(&w, "pane_width", None).contains("src/pane.h:2-2  Pane::pane_width"));
}
