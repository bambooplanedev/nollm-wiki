//! `defined` names in search: serde compatibility with an older `index.json`
//! and the whole-term match rule. The extractor always renders a defined
//! name into `## Exports`, so a "defined-only" hit can only be built by
//! hand — which is also what an index written by an older compiler looks
//! like.

use std::fs;
use tempfile::tempdir;
use wiki::query::Wiki;

fn entry(id: &str, title: &str, defined: Option<&[&str]>, methods: Option<&[&str]>) -> String {
    let list = |key: &str, v: Option<&[&str]>| match v {
        Some(d) => format!(
            ", \"{key}\": [{}]",
            d.iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => String::new(),
    };
    let defined = list("defined", defined);
    let methods = list("methods", methods);
    format!(
        "{{\"id\": \"{id}\", \"title\": \"{title}\", \"kind\": \"text\", \"aliases\": [], \
         \"summary\": null, \"pagerank\": 0.5, \"token_estimate\": 4, \
         \"neighbors_out\": [], \"neighbors_in\": []{defined}{methods}}}"
    )
}

/// Two pages: `old` has no `defined` key at all (older compiler); `manifest`
/// defines `token_estimate` but its page text never mentions it.
fn build() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let index = format!(
        "{{\"project\": \"p\", \"entries\": [{}, {}]}}",
        entry("old", "Old", None, None),
        entry("manifest", "Manifest", Some(&["token_estimate"]), None)
    );
    fs::write(dir.path().join("index.json"), index).unwrap();
    fs::write(
        dir.path().join("old.md"),
        "# Old\n\n## Body\n\nnothing here\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("manifest.md"),
        "# Manifest\n\n## Body\n\nnothing here either\n",
    )
    .unwrap();
    dir
}

#[test]
fn an_index_without_defined_still_loads() {
    let dir = build();
    let wiki = Wiki::load(dir.path()).expect("older index.json must load");
    assert!(wiki.has_page("old"));
}

#[test]
fn defined_name_matches_by_whole_term_and_word_with_no_snippet() {
    let dir = build();
    let wiki = Wiki::load(dir.path()).unwrap();
    for q in ["token_estimate", "estimate", "token"] {
        let hits = wiki.search(q, None, 10);
        assert_eq!(
            hits.len(),
            1,
            "{q}: {:?}",
            hits.iter().map(|h| &h.id).collect::<Vec<_>>()
        );
        assert_eq!(hits[0].id, "manifest", "{q}");
        assert!(
            hits[0].snippet.is_none(),
            "{q}: defined-only hit has no body match"
        );
    }
}

#[test]
fn a_substring_of_a_defined_name_is_not_a_match() {
    let dir = build();
    let wiki = Wiki::load(dir.path()).unwrap();
    // `to` ⊂ `token_estimate`, but it is neither the name nor one of its words.
    assert!(wiki.search("to", None, 10).is_empty());
    assert!(wiki.search("oken", None, 10).is_empty());
}

/// `src_query` lists `has_page` under `methods`; its page text never says
/// it. The name matches whole, not by word or prefix.
#[test]
fn a_method_name_matches_whole_only() {
    let dir = tempdir().unwrap();
    let index = format!(
        "{{\"project\": \"p\", \"entries\": [{}]}}",
        entry("src_query", "Src Query", None, Some(&["has_page"]))
    );
    fs::write(dir.path().join("index.json"), index).unwrap();
    fs::write(
        dir.path().join("src_query.md"),
        "# Src Query\n\n## Body\n\nnothing here\n",
    )
    .unwrap();
    let wiki = Wiki::load(dir.path()).unwrap();
    let ids =
        |q: &str| -> Vec<String> { wiki.search(q, None, 10).into_iter().map(|h| h.id).collect() };
    assert_eq!(ids("has_page"), vec!["src_query"]);
    assert!(
        ids("has page").is_empty(),
        "two tokens: a method is never word-split"
    );
    assert!(
        ids("has").is_empty(),
        "a prefix of a method name is not a match"
    );
}
