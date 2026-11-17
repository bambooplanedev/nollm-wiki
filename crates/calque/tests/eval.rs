//! Retrieval eval harness: one comparable score for `calque search`.
//!
//! Runs labelled queries against the frozen 17-page corpus in
//! `tests/.eval_corpus/`.
//!
//! **The corpus is frozen at commit `a55cc54` and must never be re-synced
//! with the live files it was copied from.** Re-syncing restores the moving
//! target this harness exists to remove: a score change would no longer be
//! attributable to the scorer, because the corpus could have moved instead.
//! The corpus is re-cut only by explicit decision, in a commit that quotes
//! both the old and the new table. The next
//! re-cut must add a `lib.rs`-style page whose `pub mod` lines name other
//! pages: an exports-field prototype once scored 19/19 here while
//! dropping title-as-query own-page hits on the live wiki from 35/37 to
//! 29/37 — a defect class this corpus cannot see.
//!
//! The directory is dot-prefixed so `walk` prunes it whenever ignore rules
//! are respected (`.hidden(respect_ignore)`, and `respect_ignore` defaults to
//! true) when the repository root is the walk root (keeping 17 duplicate
//! pages out of the self-hosted wiki), while compiling it *as* the walk root
//! still yields every file — the walker never filters its own root.

use calque::query::{PackBudget, Wiki};
use std::fmt::Write as _;

/// Every page the corpus must compile to, **ascending by id** — the order
/// `Wiki::list_pages` returns, which is not the source-path order the spec's
/// table uses. An `assert_eq!` on the full list rather than a count: it also
/// catches a lost extractor, an eighteenth file, and a naming-rule change
/// that moves an id. (The harness compiles with `respect_ignore: false`, as
/// `corpus_hash` walks, so a developer's global gitignore cannot eat a
/// corpus file in the first place.)
const EXPECTED_IDS: &[&str] = &[
    "architecture",
    "cache",
    "cli",
    "formats",
    "generator",
    "graph_page",
    "hash",
    "lint",
    "manifest",
    "markdown",
    "model",
    "snapshot",
    "summary",
    "text",
    "walk",
    "watch",
    "wiki",
];

/// Content hash pinning the frozen corpus, so a re-sync is caught even
/// though `corpus_compiles_to_the_expected_pages` only sees the page *set* —
/// a reviewer once copied the live `docs/ARCHITECTURE.md` over the frozen
/// copy and all other tests, snapshot included, passed unchanged. This is
/// what makes the freeze rule at the top of this file mechanically
/// enforceable rather than aspirational.
///
/// A **deliberate** re-cut updates this constant in the same commit that
/// quotes the old and new tables (see the freeze rule and the floor-ratchet
/// note above `MIN_TOP1`).
const CORPUS_HASH: &str = "3cb65763c753f33981baf851f94424817562cb1496df2fa28c866368ba5e5359";

#[path = "common/corpus.rs"]
mod corpus;

/// Compile the frozen corpus into a fresh tempdir and load it.
fn load_corpus() -> (tempfile::TempDir, Wiki) {
    let (dir, _, wiki) = corpus::compile_frozen("tests/.eval_corpus", CORPUS_HASH);
    (dir, wiki)
}

#[test]
fn corpus_compiles_to_the_expected_pages() {
    let (_dir, wiki) = load_corpus();
    let ids: Vec<String> = wiki.list_pages().into_iter().map(|(id, _)| id).collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    assert_eq!(ids, EXPECTED_IDS, "corpus did not compile as expected");
}

/// `(query, the one page that should answer it)`.
///
/// Some cases are multi-token queries where a page misses one token, kept so
/// partial matching stays covered. The set also keeps one deliberate miss
/// (`sorted` → `walk`, rank 2): a set of only passing cases is blind to the
/// next defect.
const CASES: &[(&str, &str)] = &[
    ("determinism rules", "architecture"),
    ("incremental cache", "cache"),
    ("broken wikilinks orphans", "lint"),
    ("content hash", "hash"),
    ("walk source tree sorted", "walk"),
    ("watch recompile on change", "watch"),
    ("manifest", "manifest"),
    ("synthetic corpus generator seed", "generator"),
    ("extractor registry extension dispatch", "formats"),
    ("plain txt extractor aliases", "text"),
    ("pagerank centrality damping", "graph_page"),
    ("slugify title case", "model"),
    ("one line summary fallback chain", "summary"),
    ("markdown extractor sections", "markdown"),
    ("insta redacted snapshot", "snapshot"),
    ("compile then search cli binary", "cli"),
    ("obsidian wikilink slug format", "wiki"),
    ("token estimate chars", "manifest"),
    ("cache version hash algo mismatch", "cache"),
    // `extractor trait` pins "definer over implementer". `extractor` ranks
    // `formats` first: there is no substring hit inside
    // `TextExtractor`/`MarkdownExtractor` in the `text` and `markdown`
    // signature summaries; `formats`' own score is 4.2002.
    ("extractor trait", "formats"),
    ("extractor", "formats"),
    // The deliberate miss: rank 2; `architecture` wins on body volume
    // (1.8651 vs 1.8351) and `walk`'s frozen summary is a signature, so no
    // field carries the word.
    ("sorted", "walk"),
];

/// Run every case and return `(top1, mrr@10, table)`.
///
/// The table is both the snapshot payload and the body of the floor
/// messages, so it is built once here.
///
/// Columns: the rank of the expected page (`-` when absent from the top ten),
/// the number of hits returned, and the id that actually placed first. The
/// hits count disambiguates a zero — a query that returns no hits at all
/// reads differently from one that returns ten wrong pages. The
/// rank-1 id is load-bearing: the corpus compiles into a tempdir that is
/// deleted on exit, so without it a row moving from `2` to `-` says something
/// outranked the expected page but not what.
fn score(wiki: &Wiki) -> (f64, f64, String) {
    let mut table = String::new();
    writeln!(
        table,
        "{:<39}{:<14}{:>4}{:>6}  top1_id",
        "query", "expected", "rank", "hits"
    )
    .unwrap();
    writeln!(table, "{}", "-".repeat(72)).unwrap();

    let (mut top1, mut mrr) = (0.0, 0.0);
    for (query, expected) in CASES {
        let hits = wiki.search(query, None, 10);
        let rank = hits.iter().position(|h| h.id == *expected).map(|i| i + 1);
        let top1_id = hits.first().map(|h| h.id.as_str());

        if top1_id == Some(*expected) {
            top1 += 1.0;
        }
        if let Some(r) = rank {
            mrr += 1.0 / r as f64;
        }

        writeln!(
            table,
            "{:<39}{:<14}{:>4}{:>6}  {}",
            query,
            expected,
            rank.map_or("-".to_string(), |r| r.to_string()),
            hits.len(),
            top1_id.unwrap_or("-"),
        )
        .unwrap();
    }

    let n = CASES.len() as f64;
    let (top1, mrr) = (top1 / n, mrr / n);
    write!(
        table,
        "\n{:<13}{top1:.4}\n{:<13}{mrr:.4}\n",
        "top1", "mrr@10",
    )
    .unwrap();
    (top1, mrr, table)
}

// Floors, written as exact fractions. **Never transcribe these from the
// snapshot table**: it formats with `{:.4}`, which rounds half-up, so `top1`
// prints 0.9474 against a true 0.947368… — a floor copied from the printed
// value sits *above* the baseline and fails on day one.
//
// These floors are static, so they only catch a drop below the *original*
// baseline — a commit that raises `top1`/`mrr@10` and then a later commit
// that gives the gain back both pass with both floors green, leaving only
// the snapshot diff to catch it, which is directionless (an accept-by-reflex
// waves it through). **A commit that accepts an improved table must also
// raise `MIN_TOP1`/`MIN_MRR10` to the new exact fractions, in the same
// commit** — computed the same way as below (e.g. `13.0 / 19.0`), never
// transcribed from the `{:.4}` table.
const MIN_TOP1: f64 = 21.0 / 22.0; // 0.9545454…
const MIN_MRR10: f64 = 43.0 / 44.0; // 0.9772727… = (21 + 1/2) / 22

/// The function name is load-bearing: `insta` derives the snapshot file name
/// (`eval__retrieval_quality.snap`) from it, so renaming this test orphans
/// the existing snapshot and fails CI with "no stored snapshot" instead of a
/// diff.
#[test]
fn retrieval_quality() {
    let (_dir, wiki) = load_corpus();
    let (top1, mrr, table) = score(&wiki);

    // Floors before the snapshot: a regression must fail directionally,
    // naming the number that moved, rather than as an undifferentiated
    // snapshot diff one `cargo insta accept` away from being waved through.
    //
    // Each message carries the whole table because this ordering
    // short-circuits `assert_snapshot!` — no rerun recovers it (not even with
    // INSTA_FORCE_UPDATE), and the corpus tempdir is already gone.
    assert!(
        top1 >= MIN_TOP1 - corpus::FLOOR_EPS,
        "top1 regressed: {top1} < {MIN_TOP1}\n{table}"
    );
    assert!(
        mrr >= MIN_MRR10 - corpus::FLOOR_EPS,
        "mrr@10 regressed: {mrr} < {MIN_MRR10}\n{table}"
    );

    insta::assert_snapshot!(table);
}

/// Every page's own title, typed as a query, ranks that page first. The
/// cheapest ranking invariant there is: a scorer change that breaks it has
/// let some other field outweigh a full title match (weight 3.0). 17/17 at
/// the 2026-09-02 freeze.
#[test]
fn every_page_title_ranks_its_own_page_first() {
    let (_dir, wiki) = load_corpus();
    let misses: Vec<String> = wiki
        .list_pages()
        .into_iter()
        .filter_map(|(id, title)| {
            let top = wiki
                .search(&title, None, 1)
                .into_iter()
                .next()
                .map(|h| h.id);
            (top.as_deref() != Some(id.as_str()))
                .then(|| format!("{title:?} -> {top:?}, expected {id}"))
        })
        .collect();
    assert!(
        misses.is_empty(),
        "title-as-query misses:\n{}",
        misses.join("\n")
    );
}

#[test]
fn pack_ceiling_holds_on_a_real_graph() {
    let (_dir, wiki) = load_corpus();
    let mut floor_fires = 0usize;

    for id in EXPECTED_IDS {
        for budget in [50usize, 100, 200, 400, 1000, 2000] {
            let pack = wiki
                .neighbors(
                    id,
                    1,
                    &PackBudget {
                        max_tokens: Some(budget),
                        ..Default::default()
                    },
                )
                .expect("id came from the compiled index");
            // The documented floor exception: a degraded target with no
            // neighbours admitted (`included.len() == 1`) and its own
            // degraded-block text still reporting "exceeds the budget" —
            // today that always coincides with the degraded block alone
            // exceeding the budget, but that coincidence is what this checks
            // for, not the condition itself.
            // A code target's floor is its cut outline's header and `…` line.
            let is_floor = pack.included.len() == 1
                && (pack.text.contains(calque::query::OVER_BUDGET_NOTE)
                    || pack.text.contains("— pass max_tokens:"));
            if is_floor {
                floor_fires += 1;
            }
            let tokens = calque::manifest::token_estimate(&pack.text);
            assert!(
                tokens <= budget || is_floor,
                "pack for {id} at budget {budget} is {tokens} tokens, included {:?}",
                pack.included,
            );
        }
    }

    // Pinned so a change that stops degrading targets — which would make the
    // ceiling trivially satisfiable — is visible rather than silent. 8 since
    // mention edges into code pages must be code-shaped and imports resolve
    // by module segment (2026-09-05): one more target has no admissible
    // neighbours, so its degraded block stands alone. 30 since a code
    // target is its outline (2026-10-24): at the small budgets a cut
    // outline's header and `…` line stand alone too.
    assert_eq!(
        floor_fires, 30,
        "floor exception fired an unexpected number of times"
    );
}
