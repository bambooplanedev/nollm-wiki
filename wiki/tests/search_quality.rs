//! Regression tests for the 2026-07-14 search-quality cycle: the four
//! dogfood checklist queries that failed pre-fix, plus AND semantics and
//! occurrence-graded ranking. Fixtures are authored in-test (modeled on the
//! self-hosted corpus), not a checkout of this repo.

use std::fs;
use tempfile::tempdir;
use wiki::query::Wiki;
use wiki::{compile, CompileOptions};

fn build_corpus() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    // architecture-like: the key word lives under an embedded subheading.
    fs::write(
        input.join("architecture.md"),
        "# Architecture\n\nPipeline internals.\n\n## Determinism rules\n\nDeterminism is enforced by sorted walks. Determinism again.\n",
    )
    .unwrap();
    // serve-like: multi-word query words appear apart from each other.
    fs::write(
        input.join("serve.md"),
        "# Serve\n\nThe MCP server exposes pages as resources over stdio.\n",
    )
    .unwrap();
    // lint-like: three query words scattered through the body.
    fs::write(
        input.join("lint.md"),
        "# Lint\n\nReports broken wikilinks and orphans. Links are checked pagewise.\n",
    )
    .unwrap();
    // cache-like.
    fs::write(
        input.join("cache.md"),
        "# Cache\n\nThe incremental cache skips unchanged pages on recompile.\n",
    )
    .unwrap();
    // ranking pair: many mentions vs one incidental mention. Neither title
    // contains the query word, so pre-fix both score identically (binary
    // body weight) and rank by id — which puts "aside" first. Only the
    // graded occurrence bonus can rank "hub" above it.
    fs::write(
        input.join("hub.md"),
        "# Hub\n\nkumquat kumquat kumquat kumquat kumquat.\n",
    )
    .unwrap();
    fs::write(
        input.join("aside.md"),
        "# Aside\n\nMentions a kumquat once, in passing.\n",
    )
    .unwrap();
    // cap pair: both far beyond OCCURRENCE_CAP (20), both unlinked (equal
    // pagerank) — their scores must be identical once the bonus saturates.
    fs::write(
        input.join("flood_a.md"),
        format!("# Flood A\n\n{}\n", "quokka ".repeat(25).trim()),
    )
    .unwrap();
    fs::write(
        input.join("flood_b.md"),
        format!("# Flood B\n\n{}\n", "quokka ".repeat(40).trim()),
    )
    .unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    (dir, output)
}

#[test]
fn dogfood_queries_find_their_pages() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    for (query, expected_id) in [
        ("determinism", "architecture"),
        ("MCP resources", "serve"),
        ("broken links orphans", "lint"),
        ("incremental cache", "cache"),
    ] {
        let hits = w.search(query, None, 10);
        assert!(
            hits.iter().any(|h| h.id == expected_id),
            "query {query:?} must hit {expected_id:?}, got: {:?}",
            hits.iter().map(|h| h.id.clone()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn and_semantics_require_every_token() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    // "incremental" appears only in cache.md; "orphans" only in lint.md —
    // no page contains both, so the AND query returns nothing.
    let hits = w.search("incremental orphans", None, 10);
    assert!(hits.is_empty(), "AND across tokens must yield no hit: {:?}",
        hits.iter().map(|h| h.id.clone()).collect::<Vec<_>>());
}

#[test]
fn occurrence_count_grades_ranking() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    let hits = w.search("kumquat", None, 10);
    let ids: Vec<_> = hits.iter().map(|h| h.id.as_str()).collect();
    let hub = ids.iter().position(|i| *i == "hub").expect("hub found");
    let aside = ids.iter().position(|i| *i == "aside").expect("aside found");
    assert!(hub < aside, "many-mentions page must outrank one-mention page: {ids:?}");
}

#[test]
fn occurrence_bonus_saturates_at_cap() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    let hits = w.search("quokka", None, 10);
    let a = hits.iter().find(|h| h.id == "flood_a").expect("flood_a");
    let b = hits.iter().find(|h| h.id == "flood_b").expect("flood_b");
    // 25 and 40 occurrences both saturate the cap (20); the pages are
    // structurally identical and unlinked, so their scores must tie exactly.
    assert!(
        (a.score - b.score).abs() < 1e-9,
        "scores must tie once the bonus saturates: {} vs {}",
        a.score,
        b.score
    );
}

#[test]
fn empty_and_punctuation_queries_return_nothing() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    assert!(w.search("", None, 10).is_empty());
    assert!(w.search("  ,,  ", None, 10).is_empty());
}
