//! Tests for `query`.
use super::unfence;

#[test]
fn unfence_drops_only_the_outer_fence_lines() {
    assert_eq!(unfence("```rust\nfn a() {}\n```"), "fn a() {}");
    assert_eq!(unfence("````rust\na\n```\nb\n```\n````"), "a\n```\nb\n```");
    assert_eq!(unfence("```rust\n```"), "");
    // Old, unfenced pages and unfenced text stay as they are.
    assert_eq!(unfence("fn a() {}"), "fn a() {}");
    assert_eq!(unfence("```\nquoted\n```\nmore"), "```\nquoted\n```\nmore");
}

use super::tokens::{CORPUS_STEMS, QUERY_STEMS, QUERY_STEMS_MAX};
use super::{query_dates, QueryDate, Wiki};
use crate::model::{CodeItem, CodeItemKind, PairRole};
use aho_corasick::AhoCorasick;
use std::collections::BTreeSet;

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(std::string::ToString::to_string).collect()
}

#[test]
fn stem_is_snowball_chosen_by_script_per_part() {
    for (word, stem) in [
        // English: Snowball's English algorithm.
        ("provisioning", "provis"),
        ("provision", "provis"),
        ("render", "render"),
        // Ukrainian: Snowball's Ukrainian algorithm; the fixed ending
        // list never joined `сторінок` with `сторінка`.
        ("сторінок", "сторінк"),
        ("сторінками", "сторінк"),
        ("конкурентів", "конкурент"),
        // Each alphanumeric part on its own; separators stay.
        ("mcp-сервери", "mcp-сервер"),
        ("token_estimates", "token_estim"),
    ] {
        assert_eq!(Wiki::stem(word), stem, "{word}");
    }
}

#[test]
fn field_tier_puts_exact_and_prefix_above_a_shared_stem() {
    let w = |s: &str| super::Field::new(s);
    // Exact and prefix hits score the full field weight.
    assert!((Wiki::field_tier(&w("provisioning"), "provisioning") - 1.0).abs() < 1e-9);
    assert!((Wiki::field_tier(&w("provisioning"), "provision") - 1.0).abs() < 1e-9);
    // A word form that only shares the stem scores the stem tier.
    assert!((Wiki::field_tier(&w("provision"), "provisioning") - Wiki::STEM_TIER).abs() < 1e-9);
    assert!((Wiki::field_tier(&w("сторінка"), "сторінок") - Wiki::STEM_TIER).abs() < 1e-9);
    // Every part must connect; a shared prefix alone is still not a hit.
    assert!((Wiki::field_tier(&w("rewrite"), "write") - 0.0).abs() < 1e-9);
    assert!((Wiki::field_tier(&w("soak"), "brew_provisioning") - 0.0).abs() < 1e-9);
}

#[test]
fn query_words_stay_out_of_the_corpus_stem_memo() {
    let field = super::Field::new("provision");
    assert!(
        Wiki::field_tier(&field, "quokkaprovisioning").abs() < 1e-9,
        "no hit"
    );
    assert!(CORPUS_STEMS.read().unwrap().contains_key("provision"));
    assert!(!CORPUS_STEMS
        .read()
        .unwrap()
        .contains_key("quokkaprovisioning"));
    for i in 0..=QUERY_STEMS_MAX {
        Wiki::with_query_stem(&format!("quokka{i}"), |_| ());
    }
    assert!(QUERY_STEMS.read().unwrap().len() <= QUERY_STEMS_MAX);
}

#[test]
fn body_counts_match_match_indices_and_find() {
    let tokens: Vec<String> = ["aa", "a", "ab", "zz", "aab"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let text = "aaab aab a aaaa";
    let ac = AhoCorasick::new(&tokens).unwrap();
    let (tf, first) = Wiki::body_counts(&ac, text, tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        assert_eq!(tf[i], text.match_indices(t.as_str()).count(), "{t}");
        assert_eq!(first[i], text.find(t.as_str()), "{t}");
    }
}

#[test]
fn tokenize_keeps_the_words_as_typed() {
    assert_eq!(Wiki::tokenize("агента агентом"), ["агента", "агентом"]);
    assert_eq!(
        Wiki::tokenize("MCP token_estimate"),
        ["mcp", "token_estimate"]
    );
}

#[test]
fn tokenize_lowercases_splits_and_trims_punctuation_edges() {
    assert_eq!(Wiki::tokenize("MCP resources"), vec!["mcp", "resources"]);
    assert_eq!(Wiki::tokenize("Serve,"), vec!["serve"]);
    // Interior non-alphanumerics survive.
    assert_eq!(Wiki::tokenize("source_hash"), vec!["source_hash"]);
    assert_eq!(Wiki::tokenize("code:rust"), vec!["code:rust"]);
}

#[test]
fn tokenize_dedupes_and_drops_empty() {
    assert_eq!(Wiki::tokenize("beta BETA beta"), vec!["beta"]);
    assert_eq!(Wiki::tokenize(""), [] as [String; 0]);
    assert_eq!(Wiki::tokenize("  ,, !! "), [] as [String; 0]);
}

#[test]
fn field_hit_needs_every_token_part_as_a_word_prefix() {
    let w = Wiki::field_words;
    // Words are maximal alphanumeric runs; digits count, `_ - :` split.
    assert_eq!(
        w("wiki::search 2026-09 a_b"),
        vec!["wiki", "search", "2026", "09", "a", "b"]
    );
    // Not a substring hit inside a word.
    assert!(!Wiki::field_hit(&w("rewrite"), "write"));
    assert!(!Wiki::field_hit(&w("process-wide query cache"), "id"));
    // A prefix of a word still hits (derived forms survive).
    assert!(Wiki::field_hit(&w("atomic writes"), "write"));
    assert!(Wiki::field_hit(&w("directory walk"), "dir"));
    // Every part of a punctuated token must hit; parts may match
    // non-adjacent words.
    assert!(Wiki::field_hit(
        &w("process wide query cache"),
        "process-wide"
    ));
    assert!(Wiki::field_hit(&w("search the whole wiki"), "wiki::search"));
    assert!(!Wiki::field_hit(&w("process query"), "process-wide"));
    // Unicode alphanumerics are words too.
    assert!(Wiki::field_hit(&w("naïve bayes"), "na"));
    // Longer than the word: not a prefix.
    assert!(!Wiki::field_hit(&w("ab"), "abc"));
}

#[test]
fn name_words_splits_snake_camel_and_acronym_runs() {
    assert_eq!(Wiki::name_words("PackBudget"), vec!["pack", "budget"]);
    assert_eq!(Wiki::name_words("CACHE_VERSION"), vec!["cache", "version"]);
    assert_eq!(Wiki::name_words("HTTPServer"), vec!["http", "server"]);
    assert_eq!(Wiki::name_words("has_page"), vec!["has", "page"]);
    assert_eq!(Wiki::name_words("walk"), vec!["walk"]);
}

#[test]
fn match_words_join_two_adjacent_short_words() {
    let w = |n: &str| Wiki::match_words(n);
    // `QoS` splits as `qo`, `s`; both are short, so `qos` matches too.
    assert_eq!(w("clampQoSLevel"), ["clamp", "qo", "s", "level", "qos"]);
    assert_eq!(w("QoS"), ["qo", "s", "qos"]);
    assert_eq!(w("IoT"), ["io", "t", "iot"]);
    // One long word in the pair: no join (not `getx`, `ofin`, `byid`).
    assert_eq!(w("getXPos"), ["get", "x", "pos"]);
    assert_eq!(w("find_by_id"), ["find", "by", "id", "byid"]);
    assert_eq!(w("is_of_interest"), ["is", "of", "interest", "isof"]);
    assert_eq!(w("HTTPServer"), ["http", "server"]);
    // Digits are no boundary.
    assert_eq!(w("base64Encode"), ["base64encode"]);
    assert_eq!(w("walk"), ["walk"]);
}

#[test]
fn joined_words_match_but_spelled_and_name_sets_keep_the_split() {
    use CodeItemKind::Fn;
    let items = [item(Fn, "clampQoSLevel", None, false)];
    let f = Wiki::symbol_fields("position", &items);
    assert!(f.words.contains("qos"), "{:?}", f.words);
    assert_eq!(f.name_sets, [set(&["clamp", "qo", "s", "level"])].into());
    let score = |q: &[&str]| {
        let t: Vec<String> = q.iter().map(|s| (*s).into()).collect();
        Wiki::item_score(
            &items[0],
            &t,
            &t.iter().cloned().collect(),
            &vec![1.0; t.len()],
        )
    };
    // `qos` is a name word of the item now, not its whole name.
    assert!(score(&["qos"]) > 0.0);
    assert!(score(&["qos"]) < score(&["clampqoslevel"]));
    // The joined word does not make `clamp qos level` the item's spelling.
    assert!(score(&["clamp", "qo", "s", "level"]) > score(&["clamp", "qos", "level"]));
}

fn item(kind: CodeItemKind, name: &str, doc: Option<&str>, test: bool) -> CodeItem {
    CodeItem {
        kind,
        name: name.into(),
        owner: None,
        signature: String::new(),
        start_line: 1,
        end_line: 1,
        doc: doc.map(Into::into),
        doc_text: None,
        public: false,
        nested: false,
        twin: None,
        test,
        calls: Vec::new(),
        call_lines: Vec::new(),
        call_sites: Vec::new(),
        arity: None,
        pair_role: PairRole::Neither,
        namespace: None,
        params: None,
    }
}

#[test]
fn tail_scores_only_words_the_first_sentence_misses() {
    use CodeItemKind::Fn;
    let mut a = item(Fn, "alpha", Some("Opens the gate."), false);
    let b = item(Fn, "beta", None, false);
    let score = |i: &CodeItem, q: &[&str]| {
        let t: Vec<String> = q.iter().map(|s| (*s).into()).collect();
        Wiki::item_score(i, &t, &t.iter().cloned().collect(), &vec![1.0; t.len()])
    };
    let head = score(&a, &["opens", "gate"]);
    a.doc_text = Some("Opens the gate. Refuses when a position is open.".into());
    assert!(score(&a, &["refuses", "position"]) > score(&b, &["refuses", "position"]));
    assert!((score(&a, &["opens", "gate"]) - head).abs() < 1e-12);
}

#[test]
fn old_items_json_without_doc_text_loads() {
    let json = r#"{"kind":"fn","name":"f","signature":"fn f()","start_line":1,"end_line":1,"doc":"X.","public":true}"#;
    let i: CodeItem = serde_json::from_str(json).unwrap();
    assert_eq!(i.doc_text, None);
}

#[test]
fn symbol_fields_skip_own_title_words_tests_and_modules() {
    use CodeItemKind::{Fn, Method, Module};
    let items = [
        // Another page's title word (`summary`) stays.
        item(Fn, "summary_suffix", None, false),
        item(Fn, "render", None, false),
        // Own title word goes.
        item(Fn, "cache_store", Some("Writes Atomically."), false),
        // Method words join the same field; `render` is a whole name.
        item(Method, "needs_render", None, false),
        item(Method, "suffix_of", None, false),
        item(Fn, "zebra", Some("Zebra doc."), true),
        item(Module, "formats", None, false),
    ];
    let f = Wiki::symbol_fields("cache", &items);
    assert_eq!(
        f.whole,
        set(&[
            "cache_store",
            "needs_render",
            "render",
            "suffix_of",
            "summary_suffix"
        ])
    );
    assert_eq!(f.words, set(&["needs", "of", "store", "suffix", "summary"]));
    assert_eq!(f.doc.words, ["writes", "atomically"]);
}

fn d(year: Option<u32>, month: u32, day: u32) -> QueryDate {
    QueryDate { year, month, day }
}

#[test]
fn query_dates_reads_iso_dotted_and_month_name_forms() {
    let ok = [
        ("2026-10-12", d(Some(2026), 10, 12)),
        ("12.10", d(None, 10, 12)),
        ("03.10", d(None, 10, 3)),
        ("12.10.", d(None, 10, 12)),
        ("12.10.2026", d(Some(2026), 10, 12)),
        ("12 жовтня", d(None, 10, 12)),
        ("12 Жовтня 2025", d(Some(2025), 10, 12)),
        ("жовтень 12", d(None, 10, 12)),
        ("що сталося 12 жовтня?", d(None, 10, 12)),
        ("October 12", d(None, 10, 12)),
        ("October 12, 2026", d(Some(2026), 10, 12)),
        ("12 oct", d(None, 10, 12)),
        ("sept 3", d(None, 9, 3)),
        ("3 марта", d(None, 3, 3)),
        ("1 января", d(None, 1, 1)),
        ("29 лютого", d(None, 2, 29)),
        ("29 feb 2024", d(Some(2024), 2, 29)),
    ];
    for (q, want) in ok {
        assert_eq!(query_dates(q), [want], "{q}");
    }
    for q in [
        "v1.12",
        "1.2.3",
        "0.4",
        "2026",
        "12",
        "жовтня",
        "port 8080",
        "31 лютого",
        "29 feb 2023",
        "october",
        "32 oct",
        "2026-02-30",
        "1.2",
        "12.13",
        "3.12",
        "1.10",
        "python 3.12",
        "3.11",
    ] {
        assert!(query_dates(q).is_empty(), "{q}");
    }
    assert_eq!(
        query_dates("12.10 and 12 жовтня and 5 oct"),
        [d(None, 10, 12), d(None, 10, 5)]
    );
}
