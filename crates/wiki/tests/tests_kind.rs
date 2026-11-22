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
fn code_says_when_a_test_page_was_in_its_top() {
    let (_d, w) = fixture();
    for q in ["ledger balance", "post balance", "unrelated", "ledger"] {
        for limit in 1..=5 {
            let unsplit = w.search(q, None, limit);
            let (hits, cut) = w.search_answer(q, Some(&KindFilter::Code), None, Some(limit));
            assert_eq!(cut, unsplit.iter().any(is_test), "{q:?} limit {limit}");
            assert!(!hits.iter().any(is_test));
        }
    }
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
        assert_eq!(w.in_tests_only(&missing, code, None), [q]);
        assert_eq!(w.not_found(q, None, None), Vec::<String>::new(), "{q}");
        assert_eq!(
            w.not_found(q, Some(&KindFilter::Tests), None),
            Vec::<String>::new()
        );
    }
    // A name nothing defines stays plain missing.
    let missing = w.not_found("no_such_name", code, None);
    assert_eq!(missing, ["no_such_name"]);
    assert_eq!(w.in_tests_only(&missing, code, None), Vec::<String>::new());
    // Only a `code` search points at tests.
    let named = ["only_in_tests".to_string()];
    assert_eq!(w.in_tests_only(&named, None, None), Vec::<String>::new());
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
    let t = text(&w, "only_in_tests", code);
    assert!(
        t.starts_with("only_in_tests: in test pages only; use kind \"tests\"\n"),
        "{t}"
    );
    let t = text(&w, "zq_xv only_in_tests", code);
    assert!(t.contains("no symbol named zq_xv"), "{t}");
    assert!(
        t.contains("only_in_tests: in test pages only; use kind \"tests\"\n"),
        "{t}"
    );
    assert_eq!(text(&w, "zq", code), "no matches\n");
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
    let code = cli(&out, &["ledger balance", "--kind", "code"]);
    assert!(code.ends_with(pointer), "{code}");
    assert!(!code.contains("tests/"), "{code}");
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
