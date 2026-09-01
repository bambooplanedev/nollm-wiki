//! Retrieval eval harness: one comparable score for `wiki search`.
//!
//! Runs labelled queries against the frozen 17-page corpus in
//! `tests/.eval_corpus/`.
//!
//! **The corpus is frozen at commit `a55cc54` and must never be re-synced
//! with the live files it was copied from.** Re-syncing restores the moving
//! target this harness exists to remove: a score change would no longer be
//! attributable to the scorer, because the corpus could have moved instead.
//! The corpus is re-cut only by explicit decision when a scoring cycle
//! closes, in a commit that quotes both the old and the new table.
//!
//! The directory is dot-prefixed so `walk` prunes it whenever ignore rules
//! are respected (`.hidden(respect_ignore)`, and `respect_ignore` defaults to
//! true) when the repository root is the walk root (keeping 17 duplicate
//! pages out of the self-hosted wiki), while compiling it *as* the walk root
//! still yields every file — the walker never filters its own root.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use wiki::query::{PackBudget, Wiki};
use wiki::{compile, CompileOptions};

/// Every page the corpus must compile to, **ascending by id** — the order
/// `Wiki::list_pages` returns, which is not the source-path order the spec's
/// table uses. An `assert_eq!` on the full list rather than a count: it also
/// catches a lost extractor, an eighteenth file, a naming-rule change that
/// moves an id, and a developer's global gitignore silently eating a corpus
/// file (the harness compiles with `CompileOptions::default()`, which honours
/// `git_global` and `git_exclude`).
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

/// Hash the corpus directory's relative paths and contents, in sorted path
/// order, so the result does not depend on directory-listing order.
///
/// Line endings are normalised (`\r\n` -> `\n`) before hashing: a corpus
/// checked out with CRLF moves no scoring metric, so it must not move this
/// hash either, or the check would fail spuriously on a Windows checkout.
fn corpus_hash(corpus: &Path) -> String {
    let mut files = wiki::walk::walk(corpus, false, &|_| true).unwrap();
    // `walk` already sorts by `rel_path`, but the hash's determinism is the
    // point here, not an incidental property of the walker, so make it
    // explicit rather than relying on that guarantee silently.
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for f in &files {
        let normalized = String::from_utf8_lossy(&f.bytes).replace("\r\n", "\n");
        parts.push(f.rel_path.clone().into_bytes());
        parts.push(normalized.into_bytes());
    }
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    wiki::hash::to_hex(&wiki::hash::combine(&refs))
}

/// Compile the frozen corpus into a fresh tempdir and load it.
///
/// The `TempDir` is returned alongside the `Wiki` because dropping it deletes
/// the compiled pages, and `Wiki` reads page bodies from disk on demand.
fn load_corpus() -> (TempDir, Wiki) {
    let corpus: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/.eval_corpus");
    assert_eq!(
        corpus_hash(&corpus),
        CORPUS_HASH,
        "the frozen corpus's content changed. The id-set assertion in \
         `corpus_compiles_to_the_expected_pages` cannot catch this: it only \
         sees which pages exist, not what they say. If this is a deliberate \
         re-cut, update CORPUS_HASH in the same commit that quotes the old \
         and new tables; if it is not, revert the corpus.",
    );
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out");
    compile(&corpus, &output, &CompileOptions::default()).unwrap();
    let wiki = Wiki::load(&output).unwrap();
    (dir, wiki)
}

#[test]
fn corpus_compiles_to_the_expected_pages() {
    let (_dir, wiki) = load_corpus();
    let ids: Vec<String> = wiki.list_pages().into_iter().map(|(id, _)| id).collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    assert_eq!(ids, EXPECTED_IDS, "corpus did not compile as expected");
}

/// Sections that `Wiki::search` does not score. Kept in sync with
/// `query.rs`'s `CHROME_SECTIONS` — the label guard must judge a label
/// against the text search actually reads, not the whole rendered page.
/// (Scanning the rendered page instead makes `source_hash:` in the Metadata
/// block put the token `hash` on every page.)
const CHROME_SECTIONS: [&str; 4] = ["Metadata", "Related", "Referenced By", "Notes"];

/// Minimum token length the label guard considers. Without it, `on` and
/// `line` — which occur in 16 and 15 of the 17 corpus pages — let any label
/// pass for any page.
const MIN_TOKEN_CHARS: usize = 4;

/// Queries exempt from the majority-coverage guard: deliberate minimal
/// repros for a specific fix, whose whole point is a word the target page
/// does not contain (e.g. a stemming case like `wikilinks` against a page
/// that only has `wikilink`). Empty today. Adding an entry is a claim that
/// the label is a known-unreachable target, not an oversight.
///
/// The guard this exempts from is narrower than "unreachable target": see
/// `covers_majority`'s doc comment for what it actually checks and does not.
const EXEMPT: &[&str] = &[];

/// The searchable content of a rendered page: every section except the
/// generated chrome, lowercased.
fn searchable_content(page: &str) -> String {
    wiki::rewrite::parse_sections(page)
        .into_iter()
        .filter(|(k, _)| !CHROME_SECTIONS.contains(&k.as_str()))
        .map(|(_, v)| v)
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase()
}

/// Whether at least half of `query`'s tokens of `MIN_TOKEN_CHARS` or more
/// occur in `content`. A query whose tokens are all short is unjudgeable and
/// passes.
///
/// This checks token coverage against one page **in isolation** — it never
/// compares that coverage to the other sixteen pages, so it does not detect
/// "the expected page merely exists" the way a nearest-neighbour or ranking
/// check would. Measured, it catches a mislabel pointed at a small page (few
/// tokens to coincidentally match) but **passes** a mislabel pointed at a
/// large page that happens to cover half the query's long tokens purely by
/// vocabulary size — e.g. `("incremental cache", "architecture")` and
/// `("slugify title case", "wiki")` both pass this guard today, even though
/// neither page is the answer. Repointing a failing label at a large page it
/// happens to cover is therefore a way to raise the metrics that this guard
/// does not catch.
fn covers_majority(query: &str, content: &str) -> bool {
    let toks: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .filter(|t| t.chars().count() >= MIN_TOKEN_CHARS)
        .map(str::to_string)
        .collect();
    if toks.is_empty() {
        return true;
    }
    let hit = toks.iter().filter(|t| content.contains(t.as_str())).count();
    hit * 2 >= toks.len()
}

#[test]
fn covers_majority_ignores_short_tokens_and_needs_half() {
    // Short tokens are dropped: "on" and "the" occur in nearly every page and
    // would let any label pass for any page.
    assert!(covers_majority(
        "watch on the change",
        "recompile on change"
    ));
    // Half is enough.
    assert!(covers_majority("cache version", "the cache guard"));
    // Below half fires.
    assert!(!covers_majority(
        "atomic write temp file rename",
        "renders pages and writes them"
    ));
    // A query with no long tokens cannot be judged, so it passes.
    assert!(covers_majority("a b c", ""));
}

/// `(query, the one page that should answer it)`.
///
/// Eight of these score zero today: `search` ANDs over substring `contains`,
/// so a page missing any one token is dropped before ranking. They are kept
/// deliberately — each becomes reachable under stemming or coverage-weighted
/// partial matching, which is exactly what the next scoring cycle changes. A
/// set containing only passing cases would be blind to the recall hole that
/// is search's actual defect.
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
];

#[test]
fn every_label_names_a_page_that_could_answer_it() {
    let (_dir, wiki) = load_corpus();
    for (query, expected) in CASES {
        // A label naming a page that does not exist is always a bug; a page
        // that exists but does not rank is a legitimate zero.
        assert!(
            wiki.has_page(expected),
            "label `{query}` expects page `{expected}`, which does not exist",
        );
        if EXEMPT.contains(query) {
            continue;
        }
        let page = wiki.page(expected).expect("has_page just succeeded");
        assert!(
            covers_majority(query, &searchable_content(&page)),
            "label `{query}` -> `{expected}`: fewer than half its tokens \
             appear in that page's searchable content, so the page is not \
             the answer. Fix the label, or add the query to EXEMPT if it is \
             a deliberate minimal repro.",
        );
    }
}

/// Run every case and return `(top1, mrr@10, table)`.
///
/// The table is both the snapshot payload and the body of Task 4's floor
/// messages, so it is built once here.
///
/// Columns: the rank of the expected page (`-` when absent from the top ten),
/// the number of hits returned, and the id that actually placed first. The
/// hits count disambiguates a zero — `pagerank centrality damping` returns no
/// hits at all, while the other zeros return one to seven wrong pages. The
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
// prints 0.4211 against a true 0.42105… — a floor copied from the printed
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
const MIN_TOP1: f64 = 8.0 / 19.0; // 0.4210526…
const MIN_MRR10: f64 = 55.0 / 114.0; // 0.4824561…

/// Slack on the floor comparisons. Not defensive padding — without it the
/// harness fails on its own baseline: `mrr@10` is accumulated as
/// `Σ(1.0/rank) / 19.0`, which is 0.48245614035087714, while `55.0 / 114.0`
/// evaluates to 0.48245614035087719. Same rational, different double, one ULP
/// apart. One case is worth 1/19 ≈ 5.3 percentage points, so 1e-9 cannot hide
/// a real move.
const FLOOR_EPS: f64 = 1e-9;

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
        top1 >= MIN_TOP1 - FLOOR_EPS,
        "top1 regressed: {top1} < {MIN_TOP1}\n{table}"
    );
    assert!(
        mrr >= MIN_MRR10 - FLOOR_EPS,
        "mrr@10 regressed: {mrr} < {MIN_MRR10}\n{table}"
    );

    insta::assert_snapshot!(table);
}

#[test]
fn pack_ceiling_holds_on_a_real_graph() {
    let (_dir, wiki) = load_corpus();
    let mut floor_fires = 0usize;
    let mut checked = 0usize;

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
            let is_floor = pack.included.len() == 1 && pack.text.contains("exceeds the budget");
            if is_floor {
                floor_fires += 1;
            }
            checked += 1;
            let tokens = wiki::manifest::token_estimate(&pack.text);
            assert!(
                tokens <= budget || is_floor,
                "pack for {id} at budget {budget} is {tokens} tokens, included {:?}",
                pack.included,
            );
        }
    }

    assert_eq!(checked, 102, "17 pages x 6 budgets");
    // Pinned so a change that stops degrading targets — which would make the
    // ceiling trivially satisfiable — is visible rather than silent.
    assert_eq!(
        floor_fires, 7,
        "floor exception fired an unexpected number of times"
    );
}
