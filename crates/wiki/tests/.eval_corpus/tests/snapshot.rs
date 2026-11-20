use std::fs;
use tempfile::tempdir;
use wiki::generator::generate_corpus;
use wiki::{compile, CompileOptions};

#[test]
fn rendered_page_snapshot_is_stable() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    generate_corpus(&input, 12, 42).unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();

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
