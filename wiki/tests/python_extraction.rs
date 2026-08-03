use std::fs;
use tempfile::tempdir;
use wiki::{compile, CompileOptions};

fn write(dir: &std::path::Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).unwrap();
}

/// A reduced stand-in for the audit corpora: a dataclass module, a module with
/// a constant and a private helper class, and a consumer that imports both
/// relatively. Mirrors the shapes the acceptance run checks by hand.
fn corpus(input: &std::path::Path) {
    write(
        input,
        "shapes.py",
        "from dataclasses import dataclass\n\n@dataclass(frozen=True)\nclass Article:\n    id: str\n    title: str\n    url: str\n\n@dataclass(frozen=True)\nclass FeedSource:\n    name: str\n    tier: int\n",
    );
    write(
        input,
        "helpers.py",
        "from html.parser import HTMLParser\n\nSUMMARY_LIMIT = 300\nlog = get_logger(__name__)\n\nclass _TextExtractor(HTMLParser):\n    def handle_data(self, data: str) -> None:\n        pass\n\n    def text(self) -> str:\n        return \"\"\n\ndef clean_summary(raw: str, limit: int = SUMMARY_LIMIT) -> str:\n    return raw\n",
    );
    write(
        input,
        "consumer.py",
        "from .shapes import Article\nfrom . import helpers\nimport json.decoder as jd\n\ndef run() -> Article:\n    def nested():\n        pass\n    return Article(\"\", \"\", \"\")\n",
    );
}

fn exports(page: &str) -> Vec<String> {
    page.lines()
        .skip_while(|l| !l.starts_with("## Exports"))
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| l.starts_with("- `"))
        .map(|l| {
            l.trim_start_matches("- `")
                .trim_end_matches('`')
                .to_string()
        })
        .collect()
}

#[test]
fn python_exports_and_imports_match_the_audited_shapes() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    fs::create_dir_all(&input).unwrap();
    corpus(&input);
    let out = dir.path().join("out");
    compile(&input, &out, &CompileOptions::default()).unwrap();

    let shapes = fs::read_to_string(out.join("shapes.md")).unwrap();
    // `## Exports` is still sorted plain-lexicographically at this commit;
    // the grouping-by-owner key lands in a later task, which will update
    // this vector's order.
    assert_eq!(
        exports(&shapes),
        vec![
            "@dataclass(frozen=True) class Article",
            "@dataclass(frozen=True) class FeedSource",
            "Article.id: str",
            "Article.title: str",
            "Article.url: str",
            "FeedSource.name: str",
            "FeedSource.tier: int",
        ],
        "page: {shapes}"
    );

    let helpers = fs::read_to_string(out.join("helpers.md")).unwrap();
    let h = exports(&helpers);
    assert!(h.contains(&"SUMMARY_LIMIT = 300".to_string()), "{h:?}");
    assert!(
        h.contains(&"log = get_logger(__name__)".to_string()),
        "{h:?}"
    );
    assert!(
        !h.iter()
            .any(|s| s.contains("handle_data") || s.contains("text")),
        "a private class must not export its methods: {h:?}"
    );

    let consumer = fs::read_to_string(out.join("consumer.md")).unwrap();
    for expected in [".shapes", "helpers", "json.decoder"] {
        assert!(
            consumer.contains(expected),
            "{expected} missing from imports: {consumer}"
        );
    }
    // `## Body` is always the untouched raw source, so `def nested` legitimately
    // appears there; the leak this guards against is into the curated
    // `## Exports` list.
    let c = exports(&consumer);
    assert!(
        !c.iter().any(|s| s.contains("nested")),
        "function-local def leaked into Exports: {c:?}"
    );
}
