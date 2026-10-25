//! Pins the exact rendered Markdown of one page from a seeded corpus with `insta`.

use std::fs;

mod common;

#[test]
fn rendered_page_snapshot_is_stable() {
    let (_dir, output) = common::seed_corpus(12);

    // Snapshot one deterministic page. source_hash lines are stable (content is seeded),
    // but redact them defensively so grammar/hash changes don't churn the snapshot.
    let page = fs::read_to_string(output.join("gradient_descent.md")).unwrap();
    let redacted: String = page
        .lines()
        .map(|l| {
            if l.starts_with("- source_hash:") {
                "- source_hash: <redacted>"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(redacted);
}
