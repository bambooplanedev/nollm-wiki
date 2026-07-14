use wiki::query::Wiki;
use wiki::{compile, CompileOptions};

#[test]
fn list_pages_returns_ids_and_titles_sorted() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path().join("raw");
    let out = tmp.path().join("out");
    wiki::generator::generate_corpus(&raw, 4, 42).unwrap();
    compile(&raw, &out, &CompileOptions::default()).unwrap();

    let w = Wiki::load(&out).unwrap();
    let pages = w.list_pages();
    assert_eq!(pages.len(), 4);
    // BTreeMap order: ids ascending
    let ids: Vec<&str> = pages.iter().map(|(id, _)| id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
    // every title is non-empty
    assert!(pages.iter().all(|(_, title)| !title.is_empty()));
}
