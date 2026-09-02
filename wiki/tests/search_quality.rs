//! Regression tests for search ranking.
//!
//! Coverage: the four dogfood checklist queries from the 2026-07-14 cycle,
//! plus the 2026-09-02 scoring cycle's pins — partial matching, occurrence
//! monotonicity, the heading field, and IDF over volume. Fixtures are
//! authored in-test (modeled on the self-hosted corpus), not a checkout of
//! this repo.

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
    // contains the query word, so only body term frequency can rank "hub"
    // above "aside" (which would otherwise lead on id order).
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
    // monotonicity pair: 25 vs 40 occurrences, both unlinked (equal
    // pagerank) — the higher count must score higher.
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
fn partial_match_returns_every_page_with_a_token() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    // "incremental" appears only in cache.md; "orphans" only in lint.md.
    // No page has both. Each is a hit on its single token; neither token
    // is rarer than the other, so `cache` leads on its shorter body.
    let hits = w.search("incremental orphans", None, 10);
    let ids: Vec<_> = hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(
        ids,
        ["cache", "lint"],
        "each page matching one token must be a hit"
    );
}

#[test]
fn more_occurrences_score_higher() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    let hits = w.search("quokka", None, 10);
    let a = hits.iter().find(|h| h.id == "flood_a").expect("flood_a");
    let b = hits.iter().find(|h| h.id == "flood_b").expect("flood_b");
    // 25 vs 40 occurrences on otherwise identical, unlinked pages. Each
    // page's searchable body is its H1 line plus its `quokka` run, so
    // len = tf + 3 and tf' = tf·2.2 / (tf + 1.2·(0.25 + 0.75·len/avglen))
    // is strictly increasing in tf: more mentions score strictly higher.
    // (The old scorer capped at 20 and forced an exact tie; that concept
    // is gone.)
    assert!(
        b.score > a.score,
        "40 occurrences must score above 25: {} vs {}",
        b.score,
        a.score
    );
}

#[test]
fn section_headings_are_searchable() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    // "rules" occurs only in architecture.md's `## Determinism rules`
    // heading — never in any body — so this hit comes from the heading
    // field alone and carries no snippet.
    let hits = w.search("rules", None, 10);
    let top = hits.first().expect("heading-only match must be a hit");
    assert_eq!(top.id, "architecture");
    assert!(
        top.snippet.is_none(),
        "no body occurrence, so no snippet: {:?}",
        top.snippet
    );
}

#[test]
fn rare_title_match_beats_volume_on_a_common_word() {
    // Own fixture: adding pages to `build_corpus` would shift N, df and
    // avglen under every other test in this file.
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    // The `content hash` shape from the eval corpus: a tiny page titled
    // with the rare term, versus a long page that mentions the rare term
    // many times alongside a word almost every page contains.
    fs::write(
        input.join("hash.md"),
        "# Hash\n\nBlake3 digest of a file.\n",
    )
    .unwrap();
    fs::write(
        input.join("architecture.md"),
        format!(
            "# Architecture\n\n{}\n",
            "content flows through the pipeline and each stage records a hash of its input. "
                .repeat(60)
                .trim()
        ),
    )
    .unwrap();
    for i in 0..5 {
        fs::write(
            input.join(format!("filler_{i}.md")),
            format!("# Filler {i}\n\nSome content about topic {i}.\n"),
        )
        .unwrap();
    }
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();

    let hits = w.search("content hash", None, 10);
    let ids: Vec<_> = hits.iter().map(|h| h.id.as_str()).collect();
    // Under strict AND `hash` was not even a hit (it lacks "content").
    // Under the old additive scorer `architecture` won on volume. IDF
    // makes "content" nearly weightless and the title hit on "hash"
    // decisive.
    assert_eq!(ids.first(), Some(&"hash"), "got {ids:?}");
    assert!(
        ids.contains(&"architecture"),
        "partial hit must still appear: {ids:?}"
    );
}

#[test]
fn occurrence_count_grades_ranking() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    let hits = w.search("kumquat", None, 10);
    let ids: Vec<_> = hits.iter().map(|h| h.id.as_str()).collect();
    let hub = ids.iter().position(|i| *i == "hub").expect("hub found");
    let aside = ids.iter().position(|i| *i == "aside").expect("aside found");
    assert!(
        hub < aside,
        "many-mentions page must outrank one-mention page: {ids:?}"
    );
}

#[test]
fn empty_and_punctuation_queries_return_nothing() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    assert!(w.search("", None, 10).is_empty());
    assert!(w.search("  ,,  ", None, 10).is_empty());
}

#[test]
fn body_hits_carry_a_snippet_with_context() {
    let (_dir, out) = build_corpus();
    let w = Wiki::load(&out).unwrap();
    let hits = w.search("determinism", None, 10);
    let hit = hits.iter().find(|h| h.id == "architecture").expect("hit");
    let s = hit
        .snippet
        .as_deref()
        .expect("body hit must carry a snippet");
    assert!(
        s.to_lowercase().contains("determinism"),
        "snippet shows the match: {s:?}"
    );
    // Earliest-occurrence selection: the first "determinism" in the content
    // is "Determinism is enforced by sorted walks", so its context must
    // appear in the window.
    assert!(
        s.contains("enforced"),
        "snippet centers the earliest match: {s:?}"
    );
    assert!(
        !s.contains('\n'),
        "whitespace collapsed to single spaces: {s:?}"
    );
    assert!(s.len() <= 200, "snippet stays compact: {} chars", s.len());
}

#[test]
fn title_only_hits_have_no_snippet() {
    // Needs a page whose title matches but whose *content* does not. A
    // markdown page can't provide that — its own `# H1` line lands inside
    // the rendered `## Body` section — so use a code file: the title comes
    // from the basename, which appears nowhere in the extracted content.
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("zephyr.rs"), "pub fn blow() {}\n").unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();
    let hits = w.search("zephyr", None, 10);
    let hit = hits.iter().find(|h| h.id == "zephyr").expect("hit");
    assert!(
        hit.snippet.is_none(),
        "title-only hit must have snippet None: {:?}",
        hit.snippet
    );
}

#[test]
fn snippet_never_splits_multibyte_chars() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    // Multibyte chars (— and é) crowd both sides of the match word so the
    // 60-char window edges land amid non-ASCII. Must not panic.
    let padding = "café — déjà-vu ".repeat(20);
    fs::write(
        input.join("unicode.md"),
        format!("# Unicode\n\n{padding}xylophone{padding}\n"),
    )
    .unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();
    let hits = w.search("xylophone", None, 10);
    let hit = hits.iter().find(|h| h.id == "unicode").expect("hit");
    let s = hit.snippet.as_deref().expect("snippet");
    assert!(s.contains("xylophone"));
    assert!(
        s.starts_with('…') && s.ends_with('…'),
        "both edges truncated: {s:?}"
    );
}

#[test]
fn author_heading_differing_only_in_case_from_chrome_stays_searchable() {
    // `## Notes` (exact case) is generated chrome and is skipped; an
    // author's `## NOTES` is not chrome. The exclusion is exact-case on
    // purpose so the author's section keeps both its heading and its body.
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("page.md"),
        "# Page\n\nIntro.\n\n## NOTES\n\nThe pangolin lives here.\n",
    )
    .unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();

    let body = w.search("pangolin", None, 10);
    assert_eq!(
        body.len(),
        1,
        "body under an author `## NOTES` is searchable"
    );
    assert!(body[0].snippet.is_some(), "body hit carries a snippet");

    let heading = w.search("notes", None, 10);
    assert_eq!(heading.len(), 1, "author `## NOTES` heading is scored");
    assert!(
        heading[0].snippet.is_none(),
        "heading-only hit has no snippet"
    );
}
