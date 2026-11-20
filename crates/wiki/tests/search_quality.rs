//! Regression tests for search ranking.
//!
//! Coverage: the four dogfood checklist queries, plus pins for partial
//! matching, occurrence monotonicity, the heading field, and IDF over volume. Fixtures are
//! authored in-test (modeled on the self-hosted corpus), not a checkout of
//! this repo.

use wiki::query::Wiki;

mod common;

fn build_corpus() -> (tempfile::TempDir, std::path::PathBuf) {
    let flood_a = format!("# Flood A\n\n{}\n", "quokka ".repeat(25).trim());
    let flood_b = format!("# Flood B\n\n{}\n", "quokka ".repeat(40).trim());
    let (dir, _, output) = common::compiled(&[
    // architecture-like: the key word lives under an embedded subheading.
        ("architecture.md", "# Architecture\n\nPipeline internals.\n\n## Determinism rules\n\nDeterminism is enforced by sorted walks. Determinism again.\n"),
    // serve-like: multi-word query words appear apart from each other.
        ("serve.md", "# Serve\n\nThe MCP server exposes pages as resources over stdio.\n"),
    // lint-like: three query words scattered through the body.
        ("lint.md", "# Lint\n\nReports broken wikilinks and orphans. Links are checked pagewise.\n"),
    // cache-like.
        ("cache.md", "# Cache\n\nThe incremental cache skips unchanged pages on recompile.\n"),
    // monotonicity pair: 25 vs 40 occurrences, both unlinked (equal
    // pagerank) — the higher count must score higher.
        ("flood_a.md", &flood_a),
        ("flood_b.md", &flood_b),
    ]);
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
    // (There is no occurrence cap, so no exact tie.)
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
    // The `content hash` shape from the eval corpus: a tiny page titled
    // with the rare term, versus a long page that mentions the rare term
    // many times alongside a word almost every page contains.
    let architecture = format!(
        "# Architecture\n\n{}\n",
        "content flows through the pipeline and each stage records a hash of its input. "
            .repeat(60)
            .trim()
    );
    let fillers: Vec<_> = (0..5)
        .map(|i| {
            (
                format!("filler_{i}.md"),
                format!("# Filler {i}\n\nSome content about topic {i}.\n"),
            )
        })
        .collect();
    let mut files = vec![
        ("hash.md", "# Hash\n\nBlake3 digest of a file.\n"),
        ("architecture.md", architecture.as_str()),
    ];
    files.extend(fillers.iter().map(|(n, b)| (n.as_str(), b.as_str())));
    let (_dir, _, output) = common::compiled(&files);
    let w = Wiki::load(&output).unwrap();

    let hits = w.search("content hash", None, 10);
    let ids: Vec<_> = hits.iter().map(|h| h.id.as_str()).collect();
    // Under strict AND `hash` was not even a hit (it lacks "content").
    // Volume alone would put `architecture` first. IDF
    // makes "content" nearly weightless and the title hit on "hash"
    // decisive.
    assert_eq!(ids.first(), Some(&"hash"), "got {ids:?}");
    assert!(
        ids.contains(&"architecture"),
        "partial hit must still appear: {ids:?}"
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
    let (_dir, _, output) = common::compiled(&[("zephyr.rs", "pub fn blow() {}\n")]);
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
    // Multibyte chars (— and é) crowd both sides of the match word so the
    // 60-char window edges land amid non-ASCII. Must not panic.
    let padding = "café — déjà-vu ".repeat(20);
    let (_dir, w) = common::compiled_wiki(&[(
        "unicode.md",
        &format!("# Unicode\n\n{padding}xylophone{padding}\n"),
    )]);
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
    let (_dir, _, output) = common::compiled(&[(
        "page.md",
        "# Page\n\nIntro.\n\n## NOTES\n\nThe pangolin lives here.\n",
    )]);
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

/// A `strategy_config.h` that `position_manager.h` and `criteria.h` include:
/// the pages that are titled by the query's other words, or say its file
/// name in their imports, must not outrank the file the query names.
const NAMED_FILE: &[(&str, &str)] = &[
    (
        "src/strategy/strategy_config.h",
        "struct slot_planner_params { int slot; };\n",
    ),
    (
        "src/risk/criteria.h",
        "#include \"strategy/strategy_config.h\"\n// The position criteria.\nstruct Criteria { int position; };\n",
    ),
    (
        "src/risk/position_manager.h",
        "#include \"strategy/strategy_config.h\"\n// Tracks the position.\nstruct PositionManager { int position; };\n",
    ),
    ("src/risk/limits.h", "struct Limits { int max; };\n"),
    ("src/risk/orders.h", "struct Orders { int open; };\n"),
];

fn ids(hits: &[wiki::query::Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.path.as_str()).collect()
}

#[test]
fn a_file_named_in_the_query_ranks_first() {
    // Fillers: over five pages every word is common and no idf means much.
    let fillers: Vec<_> = (0..30)
        .map(|i| (format!("src/f{i}.h"), format!("int filler_{i};\n")))
        .collect();
    let mut files = NAMED_FILE.to_vec();
    files.extend(fillers.iter().map(|(n, b)| (n.as_str(), b.as_str())));
    let (_dir, w) = common::compiled_wiki(&files);
    for q in [
        "strategy_config.h",
        "strategy_config.h include slot_planner_params criteria position",
        "Strategy_Config.H slot_planner_params criteria position",
        "src/strategy/strategy_config.h slot_planner_params criteria position",
        "strategy/strategy_config.h, slot_planner_params criteria position",
    ] {
        let hits = w.search(q, None, 10);
        assert_eq!(
            hits[0].path,
            "src/strategy/strategy_config.h",
            "{q}: {:?}",
            ids(&hits)
        );
    }
}

#[test]
fn a_path_suffix_stops_at_a_folder_and_a_word_needs_its_extension() {
    let (_dir, w) = common::compiled_wiki(NAMED_FILE);
    let hits = w.search("gy/strategy_config.h criteria position", None, 10);
    assert_ne!(hits[0].path, "src/strategy/strategy_config.h");
    // The same word with and without its extension: only the first counts.
    let with = w.search("limits orders.h", None, 1);
    assert_eq!(with[0].path, "src/risk/orders.h");
    let without = w.search("limits orders", None, 2);
    assert!(without[0].score < with[0].score);
}

#[test]
fn a_basename_on_many_pages_matches_them_all_and_a_suffix_only_one() {
    let (_dir, w) = common::compiled_wiki(&[
        ("src/manager/mod.rs", "pub fn alpha() {}\n"),
        ("src/router/mod.rs", "pub fn beta() {}\n"),
        (
            "src/alpha.rs",
            "// alpha mod.rs alpha alpha\npub fn gamma() {}\n",
        ),
        ("src/other.rs", "pub fn delta() {}\n"),
    ]);
    let hits = w.search("mod.rs beta", None, 10);
    assert_eq!(
        ids(&hits)[..2],
        ["src/router/mod.rs", "src/manager/mod.rs"],
        "both named, the rest of the score orders them"
    );
    let hits = w.search("manager/mod.rs beta", None, 10);
    assert_eq!(ids(&hits)[0], "src/manager/mod.rs");
}
