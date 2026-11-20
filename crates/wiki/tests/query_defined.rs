//! Item names in search: serde compatibility with an older `index.json` and
//! the whole-term match rule. The extractor always renders an item's name
//! into `## Exports`, so a "name-only" hit can only be built by hand.

use std::fs;
use tempfile::tempdir;
use wiki::query::Wiki;

fn entry(id: &str, title: &str, defined: Option<&[&str]>) -> String {
    let defined = defined.map_or_else(String::new, |d| {
        format!(
            ", \"defined\": [{}]",
            d.iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    format!(
        "{{\"id\": \"{id}\", \"title\": \"{title}\", \"kind\": \"text\", \"aliases\": [], \
         \"summary\": null, \"pagerank\": 0.5, \"token_estimate\": 4, \
         \"neighbors_out\": [], \"neighbors_in\": []{defined}}}"
    )
}

/// Two pages: `old` has no `defined` key at all (older compiler); `manifest`
/// has the item `token_estimate`, which its page text never mentions.
fn build() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let index = format!(
        "{{\"project\": \"p\", \"entries\": [{}, {}]}}",
        entry("old", "Old", None),
        entry("manifest", "Manifest", Some(&["token_estimate"]))
    );
    fs::write(dir.path().join("index.json"), index).unwrap();
    fs::write(
        dir.path().join("items.json"),
        r#"{"manifest": [{"kind": "fn", "name": "token_estimate", "signature": "fn token_estimate()", "start_line": 1, "end_line": 1, "doc": null, "public": true}]}"#,
    )
    .unwrap();
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
fn an_index_with_or_without_defined_loads() {
    let dir = build();
    let wiki = Wiki::load(dir.path()).expect("older index.json must load");
    assert!(wiki.has_page("old"));
}

#[test]
fn item_name_matches_by_whole_term_and_word_with_no_snippet() {
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
            "{q}: name-only hit has no body match"
        );
    }
}

#[test]
fn a_substring_of_an_item_name_is_not_a_match() {
    let dir = build();
    let wiki = Wiki::load(dir.path()).unwrap();
    // `to` ⊂ `token_estimate`, but it is neither the name nor one of its words.
    assert!(wiki.search("to", None, 10).is_empty());
    assert!(wiki.search("oken", None, 10).is_empty());
}
