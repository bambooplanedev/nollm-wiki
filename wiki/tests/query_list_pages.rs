//! `Wiki::list_pages` id and title ordering, backing the MCP server's `resources/list`.

use wiki::query::Wiki;

mod common;

#[test]
fn list_pages_returns_ids_and_titles_sorted() {
    let (_tmp, out) = common::seed_corpus(4);

    let w = Wiki::load(&out).unwrap();
    let pages = w.list_pages();
    assert_eq!(pages.len(), 4);
    // BTreeMap order: ids ascending
    let ids: Vec<&str> = pages.iter().map(|(id, _)| id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted);
    // every title is non-empty
    assert!(pages.iter().all(|(_, title)| !title.is_empty()));
}
