//! `Wiki::list_pages` id and title ordering, backing the MCP server's `resources/list`.

use wiki::query::Wiki;

mod common;

#[test]
fn list_pages_returns_ids_and_titles_sorted() {
    let (_tmp, out) = common::seed_corpus(4);

    let w = Wiki::load(&out).unwrap();
    let pages = w.list_pages();
    // Ids ascending, each with its own title (the generator names its pages
    // after their titles).
    let want = [
        ("attention_mechanism", "Attention Mechanism"),
        ("embedding_layer", "Embedding Layer"),
        ("gradient_descent", "Gradient Descent"),
        ("tokenization", "Tokenization"),
    ];
    assert_eq!(pages, want.map(|(id, t)| (id.to_string(), t.to_string())));
}
