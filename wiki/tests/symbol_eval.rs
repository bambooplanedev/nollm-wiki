//! Symbol eval harness: scores labelled queries by the *symbol* that answers
//! them, not the page.
//!
//! Runs against the frozen corpus in `tests/.symbol_corpus/` (`src/`,
//! `docs/ARCHITECTURE.md`, `README.md`).
//!
//! **The corpus was frozen on 2026-10-22 at `a81a332`; its files are this
//! crate's sources as of `7209ee2`** (identical to `777b30a` except
//! `src/serve.rs`, which already has `search_json`). The SHAs may dangle after
//! a history squash; `SYMBOL_CORPUS_HASH` is the real pin. It must never be re-synced with the live files it was
//! copied from: a score change would no longer be attributable to the scorer,
//! because the corpus could have moved instead. It is re-cut only by explicit
//! decision, in a commit that quotes both the old and the new table.
//! Re-cut 2026-10-23: two test fixtures' text made English (`extract_python.rs`,
//! `rewrite.rs`), line counts kept; the table did not change.
//!
//! **Held-out rule:** every third case in each class is `held_out`. Held-out
//! cases are never used to choose constants (ranking weights, thresholds);
//! only the `tune` split may inform them. The split is pinned by
//! `held_out_is_every_third_case_per_class`.
//!
//! A case is scored by its file and by its symbol: `file_rank` is the first
//! hit whose source path equals the case's path; `symbol_rank` is the position
//! of the expected `(path, symbol)` among the response's symbol lines in
//! output order (what the agent reads first), `symbol_hit@3` that rank ≤ 3.
//!
//! The `tokens` column charges the MCP tool's default search response
//! (`search_lines` at `DEFAULT_SEARCH_LIMIT`, 5; it was 639.9 / 700.7
//! tune / held at limit 10); `tokens_json` in the footers charges the
//! `format: "json"` response (`search_json`) at the same limit. Ranks are
//! still taken at limit 10. A step that changes the default renderer or
//! limit must swap it here and quote both numbers.

use regex::Regex;
use std::fmt::Write as _;
use std::path::Path;
use wiki::manifest::token_estimate;
use wiki::query::{PackBudget, Wiki, OVER_BUDGET_NOTE};
use wiki::serve::{search_json, search_lines, DEFAULT_NEIGHBORS_MAX_TOKENS, DEFAULT_SEARCH_LIMIT};

#[path = "common/corpus.rs"]
mod corpus;

/// Content hash pinning the frozen corpus (see `common/corpus.rs`).
const SYMBOL_CORPUS_HASH: &str = "e3637a4e66af876b1cd3755477d96e84b03b2854f620a6d7038c0c64b05259e8";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Class {
    Name,
    NameWords,
    DocOnly,
    Stopwords,
    MethodSplit,
    V7,
    TestTarget,
}

const CLASSES: &[Class] = &[
    Class::Name,
    Class::NameWords,
    Class::DocOnly,
    Class::Stopwords,
    Class::MethodSplit,
    Class::V7,
    Class::TestTarget,
];

struct Case {
    query: &'static str,
    path: &'static str,
    symbol: &'static str,
    class: Class,
    held_out: bool,
}

const fn case(
    query: &'static str,
    path: &'static str,
    symbol: &'static str,
    class: Class,
    held_out: bool,
) -> Case {
    Case {
        query,
        path,
        symbol,
        class,
        held_out,
    }
}

const CASES: &[Case] = &[
    case(
        "lang_for_ext",
        "src/formats/code.rs",
        "lang_for_ext",
        Class::Name,
        false,
    ),
    case(
        "strip_rust_test_modules",
        "src/formats/extract_rust.rs",
        "strip_rust_test_modules",
        Class::Name,
        false,
    ),
    case("field_hit", "src/query.rs", "field_hit", Class::Name, true),
    case(
        "disambiguate_ids",
        "src/lib.rs",
        "disambiguate_ids",
        Class::Name,
        false,
    ),
    case(
        "module_stem",
        "src/formats/code.rs",
        "module_stem",
        Class::Name,
        false,
    ),
    case(
        "source_rank",
        "src/formats/code.rs",
        "source_rank",
        Class::Name,
        true,
    ),
    case(
        "PackBudget",
        "src/query.rs",
        "PackBudget",
        Class::Name,
        false,
    ),
    case(
        "CodeExtractor",
        "src/formats/code.rs",
        "CodeExtractor",
        Class::Name,
        false,
    ),
    case(
        "token_estimate",
        "src/manifest.rs",
        "token_estimate",
        Class::Name,
        true,
    ),
    case(
        "snippet built",
        "src/query.rs",
        "snippet",
        Class::NameWords,
        false,
    ),
    case(
        "split frontmatter",
        "src/formats/markdown.rs",
        "split_frontmatter",
        Class::NameWords,
        false,
    ),
    case(
        "first sentence of a summary line",
        "src/formats/summary.rs",
        "first_sentence",
        Class::NameWords,
        true,
    ),
    case(
        "name words of an identifier",
        "src/query.rs",
        "name_words",
        Class::NameWords,
        false,
    ),
    case(
        "content text of page sections",
        "src/query.rs",
        "content_text",
        Class::NameWords,
        false,
    ),
    case(
        "attribute value of a tag",
        "src/formats/extract_vue.rs",
        "attr_value",
        Class::NameWords,
        true,
    ),
    case(
        "excerpt around the earliest occurrence",
        "src/query.rs",
        "snippet",
        Class::DocOnly,
        false,
    ),
    case(
        "temp file rename half-written page",
        "src/rewrite.rs",
        "write_atomic",
        Class::DocOnly,
        false,
    ),
    case(
        "html elements that never take a closing tag",
        "src/formats/extract_vue.rs",
        "is_void",
        Class::DocOnly,
        true,
    ),
    case(
        "ids within hops along either edge direction",
        "src/query.rs",
        "bfs_neighborhood",
        Class::DocOnly,
        false,
    ),
    case(
        "path outside output real source change",
        "src/watch.rs",
        "event_is_relevant",
        Class::DocOnly,
        false,
    ),
    case(
        "lazily reloading handle loaded snapshot",
        "src/serve.rs",
        "WikiState",
        Class::DocOnly,
        true,
    ),
    case(
        "where is the snippet built",
        "src/query.rs",
        "snippet",
        Class::Stopwords,
        false,
    ),
    case(
        "how do we write a page atomically",
        "src/rewrite.rs",
        "write_atomic",
        Class::Stopwords,
        false,
    ),
    case(
        "what is the module stem of a path",
        "src/formats/code.rs",
        "module_stem",
        Class::Stopwords,
        true,
    ),
    case(
        "where are the test modules stripped",
        "src/formats/extract_rust.rs",
        "strip_rust_test_modules",
        Class::Stopwords,
        false,
    ),
    case(
        "needs render",
        "src/cache.rs",
        "needs_render",
        Class::MethodSplit,
        false,
    ),
    case(
        "retain ids",
        "src/cache.rs",
        "retain_ids",
        Class::MethodSplit,
        false,
    ),
    case(
        "with defaults",
        "src/formats/mod.rs",
        "with_defaults",
        Class::MethodSplit,
        true,
    ),
    case(
        "resolve segment",
        "src/graph.rs",
        "resolve_segment",
        Class::MethodSplit,
        false,
    ),
    case(
        "resolve include",
        "src/graph.rs",
        "resolve_include",
        Class::MethodSplit,
        false,
    ),
    case(
        "has page",
        "src/query.rs",
        "has_page",
        Class::MethodSplit,
        true,
    ),
    case(
        "list pages",
        "src/query.rs",
        "list_pages",
        Class::MethodSplit,
        false,
    ),
    case(
        "content text",
        "src/query.rs",
        "content_text",
        Class::MethodSplit,
        false,
    ),
    case(
        "name words",
        "src/query.rs",
        "name_words",
        Class::MethodSplit,
        true,
    ),
    case(
        "field words",
        "src/query.rs",
        "field_words",
        Class::MethodSplit,
        false,
    ),
    case(
        "field hit",
        "src/query.rs",
        "field_hit",
        Class::MethodSplit,
        false,
    ),
    case(
        "defined terms",
        "src/query.rs",
        "defined_terms",
        Class::MethodSplit,
        true,
    ),
    case(
        "bfs neighborhood",
        "src/query.rs",
        "bfs_neighborhood",
        Class::MethodSplit,
        false,
    ),
    case(
        "pagerank of",
        "src/query.rs",
        "pagerank_of",
        Class::MethodSplit,
        false,
    ),
    case(
        "with wiki",
        "src/serve.rs",
        "with_wiki",
        Class::MethodSplit,
        true,
    ),
    case(
        "list resources",
        "src/serve.rs",
        "list_resources",
        Class::MethodSplit,
        false,
    ),
    case(
        "read resource",
        "src/serve.rs",
        "read_resource",
        Class::MethodSplit,
        false,
    ),
    case(
        "next u64",
        "src/generator.rs",
        "next_u64",
        Class::MethodSplit,
        true,
    ),
    case(
        "render fingerprint",
        "src/rewrite.rs",
        "render_fingerprint",
        Class::V7,
        false,
    ),
    case(
        "render page",
        "src/rewrite.rs",
        "render_page",
        Class::V7,
        false,
    ),
    case(
        "render index json",
        "src/manifest.rs",
        "render_index_json",
        Class::V7,
        true,
    ),
    case(
        "render span",
        "src/formats/code.rs",
        "render_span",
        Class::V7,
        false,
    ),
    case("resolve", "src/graph.rs", "resolve", Class::V7, false),
    case("render", "src/rewrite.rs", "render_page", Class::V7, true),
    case(
        "a stray backtick does not re pair later lines",
        "src/rewrite.rs",
        "a_stray_backtick_does_not_re_pair_later_lines",
        Class::TestTarget,
        false,
    ),
    case(
        "pagerank is deterministic and sums to about one",
        "src/graph.rs",
        "pagerank_is_deterministic_and_sums_to_about_one",
        Class::TestTarget,
        false,
    ),
    case(
        "stale page files lists orphaned on disk only",
        "src/lib.rs",
        "stale_page_files_lists_orphaned_on_disk_only",
        Class::TestTarget,
        true,
    ),
];

fn corpus_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/.symbol_corpus")
}

fn read_corpus_file(path: &str) -> String {
    std::fs::read_to_string(corpus_dir().join(path))
        .unwrap_or_else(|e| panic!("corpus file `{path}` unreadable: {e}"))
}

/// Split an identifier like `Wiki::name_words`: on `_`/`-` and camel
/// boundaries, acronym runs kept together.
/// Deliberate copy of the private `Wiki::name_words`; drift would silently
/// weaken the `DocOnly` guard.
fn name_words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut words = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let boundary = c.is_uppercase()
            && i > 0
            && (chars[i - 1].is_lowercase()
                || (chars[i - 1].is_uppercase()
                    && chars.get(i + 1).is_some_and(|n| n.is_lowercase())));
        if boundary && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Matches a Rust definition of `symbol`: the check that a label's symbol
/// really is defined in its file, and the line an item must cover.
fn definition_re(symbol: &str) -> Regex {
    Regex::new(&format!(
        r"\b(fn|struct|enum|trait|type|const|static|mod)\s+{}\b",
        regex::escape(symbol)
    ))
    .unwrap()
}

#[test]
fn every_label_names_a_symbol_defined_in_its_file() {
    assert!(!CASES.is_empty(), "no cases");
    for c in CASES {
        let text = read_corpus_file(c.path);
        let re = definition_re(c.symbol);
        assert!(
            re.is_match(&text),
            "label `{}` expects `{}` defined in `{}`, but no definition found",
            c.query,
            c.symbol,
            c.path
        );
    }
}

/// Words too common to count as a match on a symbol's name.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "in", "on", "is", "are", "and", "or", "for", "by", "with",
    "that", "it", "its", "we", "do", "how", "what", "where", "which",
];

#[test]
fn doc_only_queries_do_not_match_the_symbol_name() {
    let mut seen = 0;
    for c in CASES.iter().filter(|c| c.class == Class::DocOnly) {
        seen += 1;
        let words = name_words(c.symbol);
        let parts: Vec<String> = c
            .query
            .to_lowercase()
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        for t in parts.iter().filter(|t| !STOPWORDS.contains(&t.as_str())) {
            assert!(
                !words.iter().any(|w| w.starts_with(t.as_str())),
                "DocOnly label `{}`: token `{t}` is a prefix of a word of `{}`",
                c.query,
                c.symbol
            );
        }
        let toks: Vec<&String> = parts.iter().filter(|t| t.chars().count() >= 4).collect();
        let text = read_corpus_file(c.path).to_lowercase();
        let hit = toks.iter().filter(|t| text.contains(t.as_str())).count();
        assert!(
            hit * 2 >= toks.len(),
            "DocOnly label `{}`: fewer than half its tokens occur in `{}`",
            c.query,
            c.path
        );
    }
    assert!(seen > 0, "no DocOnly cases");
}

#[test]
fn held_out_is_every_third_case_per_class() {
    for &class in CLASSES {
        assert!(
            CASES.iter().any(|c| c.class == class),
            "{class:?} has no cases"
        );
        for (i, c) in CASES.iter().filter(|c| c.class == class).enumerate() {
            assert_eq!(
                c.held_out,
                i % 3 == 2,
                "{class:?} case #{i} `{}`: held_out must be every third case",
                c.query
            );
        }
    }
}

fn split(held_out: bool) -> &'static str {
    if held_out {
        "held"
    } else {
        "tune"
    }
}

/// Page id -> source path, from the compiled `index.json`.
fn id_paths(out: &Path) -> std::collections::BTreeMap<String, String> {
    corpus::read_json(out, "index.json")["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_string(),
                e["path"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// Per-split aggregates: `(file_hit@1, file_hit@3, mrr@10, symbol_hit@3,
/// mean tokens, n)`.
#[derive(Default)]
struct Agg {
    h1: f64,
    h3: f64,
    mrr: f64,
    s3: f64,
    tokens: f64,
    tokens_json: f64,
    n: f64,
}

/// Whether a file or symbol rank counts as a `hit@3`.
fn hit3(rank: Option<usize>) -> bool {
    rank.is_some_and(|r| r <= 3)
}

impl Agg {
    fn add(
        &mut self,
        rank: Option<usize>,
        symbol_rank: Option<usize>,
        tokens: usize,
        tokens_json: usize,
    ) {
        self.n += 1.0;
        self.tokens += tokens as f64;
        self.tokens_json += tokens_json as f64;
        if hit3(symbol_rank) {
            self.s3 += 1.0;
        }
        if let Some(r) = rank {
            self.mrr += 1.0 / r as f64;
            if r == 1 {
                self.h1 += 1.0;
            }
        }
        if hit3(rank) {
            self.h3 += 1.0;
        }
    }

    fn footer(&self, name: &str) -> String {
        format!(
            "{name:<5} file_hit@1={}/{} ({:.4}) file_hit@3={}/{} ({:.4}) mrr@10={:.4} symbol_hit@3={}/{} ({:.4}) tokens={:.1} tokens_json={:.1}\n",
            self.h1,
            self.n,
            self.h1 / self.n,
            self.h3,
            self.n,
            self.h3 / self.n,
            self.mrr / self.n,
            self.s3,
            self.n,
            self.s3 / self.n,
            self.tokens / self.n,
            self.tokens_json / self.n,
        )
    }
}

/// Run every case; return `(tune, held, (own-title hits, pages), table)`.
fn score(out: &Path, wiki: &Wiki) -> (Agg, Agg, (usize, usize), String) {
    let paths = id_paths(out);
    let mut table = String::new();
    writeln!(
        table,
        "{:<50}{:<13}{:<6}{:>10}{:>12}{:>8}  top1_path",
        "query", "class", "split", "file_rank", "symbol_rank", "tokens"
    )
    .unwrap();
    writeln!(table, "{}", "-".repeat(112)).unwrap();

    let (mut tune, mut held) = (Agg::default(), Agg::default());
    let mut ranks: Vec<(Class, Option<usize>, Option<usize>)> = Vec::new();
    for c in CASES {
        let hits = wiki.search(c.query, None, 10);
        let rank = hits.iter().position(|h| h.path == c.path).map(|i| i + 1);
        // The symbol line's name is `owner::name`; the label is the bare name.
        let symbol_rank = hits
            .iter()
            .flat_map(|h| &h.symbols)
            .position(|s| s.path == c.path && s.name.rsplit("::").next() == Some(c.symbol))
            .map(|i| i + 1);
        let top1 = hits.first().map_or("-".to_string(), |h| h.path.clone());
        let sent = wiki.search(c.query, None, DEFAULT_SEARCH_LIMIT);
        let tokens = token_estimate(&search_lines(&sent));
        let tokens_json = token_estimate(&search_json(sent));
        (if c.held_out { &mut held } else { &mut tune }).add(
            rank,
            symbol_rank,
            tokens,
            tokens_json,
        );
        ranks.push((c.class, rank, symbol_rank));
        let show = |r: Option<usize>| r.map_or("-".to_string(), |r| r.to_string());
        writeln!(
            table,
            "{:<50}{:<13}{:<6}{:>10}{:>12}{:>8}  {top1}",
            c.query,
            format!("{:?}", c.class),
            split(c.held_out),
            show(rank),
            show(symbol_rank),
            tokens,
        )
        .unwrap();
    }

    writeln!(table).unwrap();
    table.push_str(&tune.footer("tune"));
    table.push_str(&held.footer("held"));
    for class in CLASSES {
        let of: Vec<_> = ranks.iter().filter(|(c, _, _)| c == class).collect();
        let h3 = of.iter().filter(|(_, r, _)| hit3(*r)).count();
        let s3 = of.iter().filter(|(_, _, s)| hit3(*s)).count();
        let n = of.len();
        writeln!(
            table,
            "class {class:?}: file_hit@3={h3}/{n} symbol_hit@3={s3}/{n}"
        )
        .unwrap();
    }

    let code_id = paths
        .iter()
        .find(|(_, p)| p.as_str() == "src/formats/code.rs")
        .map(|(id, _)| id.clone())
        .expect("src/formats/code.rs compiles to a page");
    let pack = wiki
        .neighbors(
            &code_id,
            1,
            &PackBudget {
                max_tokens: Some(DEFAULT_NEIGHBORS_MAX_TOKENS),
                ..Default::default()
            },
        )
        .expect("id came from the compiled index");
    writeln!(
        table,
        "neighbors code.rs: tokens={} target_degraded={}",
        token_estimate(&pack.text),
        pack.text.contains(OVER_BUDGET_NOTE),
    )
    .unwrap();
    let outline = wiki
        .outline(&code_id, Some(DEFAULT_NEIGHBORS_MAX_TOKENS))
        .expect("id came from the compiled index");
    let cut = outline
        .lines()
        .last()
        .and_then(|l| l.strip_prefix("… "))
        .unwrap_or("none");
    writeln!(
        table,
        "page outline code.rs: tokens={} cut={cut}",
        token_estimate(&outline),
    )
    .unwrap();

    // Recorded, not asserted: this is the self-hosted code the design names.
    let pages = wiki.list_pages();
    let own = pages
        .iter()
        .filter(|(id, title)| {
            wiki.search(title, None, 1)
                .first()
                .is_some_and(|h| h.id == *id)
        })
        .count();
    writeln!(table, "title_as_query: {own}/{}", pages.len()).unwrap();
    (tune, held, (own, pages.len()), table)
}

// Floors, exact fractions of case counts. Never transcribe from the `{:.4}`
// table (it rounds). Ratchet them in the commit that accepts an improvement.
// Every class counts since step 4 (the floors were reset when the method,
// V7 and test-target classes joined the splits). Tune symbol_hit@3 was 35/35
// until the stem tier (2026-10-31): "content text of page sections" now ranks
// rewrite.rs first, whose summary says "section parsing", and its symbol
// fourth; a real ambiguity the stem tier exposes, kept as the one known miss.
const MIN_FILE_HIT3_TUNE: f64 = 1.0; // 35/35
const MIN_FILE_HIT3_HELD: f64 = 1.0; // 17/17
const MIN_SYMBOL_HIT3_TUNE: f64 = 34.0 / 35.0;
const MIN_SYMBOL_HIT3_HELD: f64 = 1.0; // 17/17
const MIN_TITLE_AS_QUERY: f64 = 1.0; // 28/28

/// The function name is load-bearing: `insta` derives the snapshot file name
/// (`symbol_eval__symbol_retrieval.snap`) from it.
#[test]
fn symbol_retrieval() {
    let (_dir, out, wiki) = corpus::compile_frozen("tests/.symbol_corpus", SYMBOL_CORPUS_HASH);
    let (tune, held, (own, pages), table) = score(&out, &wiki);

    // Floors before the snapshot, with the table in the message: this
    // ordering short-circuits `assert_snapshot!`.
    assert!(
        tune.h3 / tune.n >= MIN_FILE_HIT3_TUNE - corpus::FLOOR_EPS,
        "tune file_hit@3 regressed below {MIN_FILE_HIT3_TUNE}\n{table}"
    );
    assert!(
        held.h3 / held.n >= MIN_FILE_HIT3_HELD - corpus::FLOOR_EPS,
        "held file_hit@3 regressed below {MIN_FILE_HIT3_HELD}\n{table}"
    );
    assert!(
        tune.s3 / tune.n >= MIN_SYMBOL_HIT3_TUNE - corpus::FLOOR_EPS,
        "tune symbol_hit@3 regressed below {MIN_SYMBOL_HIT3_TUNE}\n{table}"
    );
    assert!(
        held.s3 / held.n >= MIN_SYMBOL_HIT3_HELD - corpus::FLOOR_EPS,
        "held symbol_hit@3 regressed below {MIN_SYMBOL_HIT3_HELD}\n{table}"
    );

    assert!(
        own as f64 / pages as f64 >= MIN_TITLE_AS_QUERY - corpus::FLOOR_EPS,
        "title_as_query regressed: {own}/{pages}\n{table}"
    );

    insta::assert_snapshot!(table);
}

/// An item's lines are what an agent reads instead of the whole file, so a
/// range that misses its own item sends the agent to the wrong code.
#[test]
fn items_on_the_frozen_corpus_are_line_true() {
    let (_dir, out, _wiki) = corpus::compile_frozen("tests/.symbol_corpus", SYMBOL_CORPUS_HASH);
    let (index, all_items) = (
        corpus::read_json(&out, "index.json"),
        corpus::read_json(&out, "items.json"),
    );
    let entries = index["entries"].as_array().unwrap();
    let items_of = |entry: &serde_json::Value| all_items[entry["id"].as_str().unwrap()].as_array();
    let line_of =
        |item: &serde_json::Value, key: &str| usize::try_from(item[key].as_u64().unwrap()).unwrap();

    let mut checked = 0;
    for entry in entries {
        let Some(items) = items_of(entry) else {
            continue;
        };
        let path = entry["path"].as_str().unwrap();
        let text = read_corpus_file(path);
        let lines: Vec<&str> = text.lines().collect();
        for item in items {
            let name = item["name"].as_str().unwrap();
            let (start, end) = (line_of(item, "start_line"), line_of(item, "end_line"));
            assert!(
                1 <= start && start <= end && end <= lines.len(),
                "{path}: {item} is outside the file's {} lines",
                lines.len()
            );
            assert!(
                lines[start - 1..end].iter().any(|l| l.contains(name)),
                "{path}: lines {start}-{end} do not contain `{name}`"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no items on the frozen corpus");

    for c in CASES {
        let entry = entries
            .iter()
            .find(|e| e["path"] == c.path)
            .unwrap_or_else(|| panic!("no page for `{}`", c.path));
        let text = read_corpus_file(c.path);
        let at = definition_re(c.symbol).find(&text).unwrap().start();
        let line = text[..at].matches('\n').count() + 1;
        let item = items_of(entry)
            .unwrap_or_else(|| panic!("`{}` has no items", c.path))
            .iter()
            .find(|i| {
                i["name"] == c.symbol
                    && line_of(i, "start_line") <= line
                    && line <= line_of(i, "end_line")
            })
            .unwrap_or_else(|| panic!("`{}`: no item `{}` covers line {line}", c.path, c.symbol));
        if c.class == Class::DocOnly {
            assert!(
                item["doc"].is_string(),
                "DocOnly `{}`: item `{}` has no doc",
                c.query,
                c.symbol
            );
        }
    }
}
