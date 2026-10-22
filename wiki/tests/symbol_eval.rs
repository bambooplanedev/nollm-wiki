//! Symbol eval harness: scores labelled queries by the *symbol* that answers
//! them, not the page.
//!
//! Runs against the frozen corpus in `tests/.symbol_corpus/` (`src/`,
//! `docs/ARCHITECTURE.md`, `README.md`).
//!
//! **The corpus is frozen at `a81a332`; its files are this crate's sources as
//! of `7209ee2`** (identical to `777b30a` except `src/serve.rs`, which already
//! has `search_json`). It must never be re-synced with the live files it was
//! copied from: a score change would no longer be attributable to the scorer,
//! because the corpus could have moved instead. It is re-cut only by explicit
//! decision, in a commit that quotes both the old and the new table.
//!
//! **Held-out rule:** every third case in each class is `held_out`. Held-out
//! cases are never used to choose constants (ranking weights, thresholds);
//! only the `tune` split may inform them. The split is pinned by
//! `held_out_is_every_third_case_per_class`.
//!
//! Until symbol hits exist, a case is scored by the expected symbol's *file*:
//! `file_rank` is the first hit whose source path equals the case's path.

use regex::Regex;
use std::fmt::Write as _;
use std::path::Path;
use wiki::manifest::token_estimate;
use wiki::query::{PackBudget, Wiki, OVER_BUDGET_NOTE};
use wiki::serve::{search_json, DEFAULT_NEIGHBORS_MAX_TOKENS};

#[path = "common/corpus.rs"]
mod corpus;

/// Content hash pinning the frozen corpus (see `common/corpus.rs`).
const SYMBOL_CORPUS_HASH: &str = "ef2ee16ead15e4a3b6c5e9a89a1c293f08f0f82de631d4ed13f78c5580311c89";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Class {
    Name,
    NameWords,
    DocOnly,
    Stopwords,
}

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

#[test]
fn every_label_names_a_symbol_defined_in_its_file() {
    assert!(!CASES.is_empty(), "no cases");
    for c in CASES {
        let text = read_corpus_file(c.path);
        let re = Regex::new(&format!(
            r"\b(fn|struct|enum|trait|type|const|static|mod)\s+{}\b",
            regex::escape(c.symbol)
        ))
        .unwrap();
        assert!(
            re.is_match(&text),
            "label `{}` expects `{}` defined in `{}`, but no definition found",
            c.query,
            c.symbol,
            c.path
        );
    }
}

#[test]
fn doc_only_queries_do_not_match_the_symbol_name() {
    let mut seen = 0;
    for c in CASES.iter().filter(|c| c.class == Class::DocOnly) {
        seen += 1;
        let words = name_words(c.symbol);
        let toks: Vec<String> = c
            .query
            .to_lowercase()
            .split_whitespace()
            .filter(|t| t.chars().count() >= 4)
            .map(str::to_string)
            .collect();
        for t in &toks {
            assert!(
                !words.iter().any(|w| w.starts_with(t.as_str())),
                "DocOnly label `{}`: token `{t}` is a prefix of a word of `{}`",
                c.query,
                c.symbol
            );
        }
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
    for class in [
        Class::Name,
        Class::NameWords,
        Class::DocOnly,
        Class::Stopwords,
    ] {
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
fn id_paths(out: &Path) -> std::collections::HashMap<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("index.json")).unwrap()).unwrap();
    v["entries"]
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

/// Per-split aggregates: `(file_hit@1, file_hit@3, mrr@10, mean tokens, n)`.
#[derive(Default)]
struct Agg {
    h1: f64,
    h3: f64,
    mrr: f64,
    tokens: f64,
    n: f64,
}

impl Agg {
    fn add(&mut self, rank: Option<usize>, tokens: usize) {
        self.n += 1.0;
        self.tokens += tokens as f64;
        if let Some(r) = rank {
            self.mrr += 1.0 / r as f64;
            if r == 1 {
                self.h1 += 1.0;
            }
            if r <= 3 {
                self.h3 += 1.0;
            }
        }
    }

    fn footer(&self, name: &str) -> String {
        format!(
            "{name:<5} file_hit@1={:.4} file_hit@3={:.4} mrr@10={:.4} tokens={:.1}\n",
            self.h1 / self.n,
            self.h3 / self.n,
            self.mrr / self.n,
            self.tokens / self.n,
        )
    }
}

/// Run every case; return `(tune, held, table)`.
fn score(out: &Path, wiki: &Wiki) -> (Agg, Agg, String) {
    let paths = id_paths(out);
    let mut table = String::new();
    writeln!(
        table,
        "{:<46}{:<11}{:<6}{:>10}{:>8}  top1_path",
        "query", "class", "split", "file_rank", "tokens"
    )
    .unwrap();
    writeln!(table, "{}", "-".repeat(100)).unwrap();

    let (mut tune, mut held) = (Agg::default(), Agg::default());
    for c in CASES {
        let hits = wiki.search(c.query, None, 10);
        let hit_paths: Vec<&str> = hits.iter().map(|h| paths[&h.id].as_str()).collect();
        let rank = hit_paths.iter().position(|p| *p == c.path).map(|i| i + 1);
        let tokens = token_estimate(&search_json(hits));
        (if c.held_out { &mut held } else { &mut tune }).add(rank, tokens);
        writeln!(
            table,
            "{:<46}{:<11}{:<6}{:>10}{:>8}  {}",
            c.query,
            format!("{:?}", c.class),
            split(c.held_out),
            rank.map_or("-".to_string(), |r| r.to_string()),
            tokens,
            hit_paths.first().copied().unwrap_or("-"),
        )
        .unwrap();
    }

    writeln!(table).unwrap();
    table.push_str(&tune.footer("tune"));
    table.push_str(&held.footer("held"));

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
    (tune, held, table)
}

// Floors, exact fractions of case counts. Never transcribe from the `{:.4}`
// table (it rounds). Ratchet them in the commit that accepts an improvement.
const MIN_FILE_HIT3_TUNE: f64 = 14.0 / 15.0;
const MIN_FILE_HIT3_HELD: f64 = 6.0 / 7.0;

/// Slack on the floor comparisons (accumulated-sum ULP noise), as in `eval.rs`.
const FLOOR_EPS: f64 = 1e-9;

/// The function name is load-bearing: `insta` derives the snapshot file name
/// (`symbol_eval__symbol_retrieval.snap`) from it.
#[test]
fn symbol_retrieval() {
    let (_dir, out, wiki) = corpus::compile_frozen("tests/.symbol_corpus", SYMBOL_CORPUS_HASH);
    let (tune, held, table) = score(&out, &wiki);

    // Floors before the snapshot, with the table in the message: this
    // ordering short-circuits `assert_snapshot!`.
    assert!(
        tune.h3 / tune.n >= MIN_FILE_HIT3_TUNE - FLOOR_EPS,
        "tune file_hit@3 regressed below {MIN_FILE_HIT3_TUNE}\n{table}"
    );
    assert!(
        held.h3 / held.n >= MIN_FILE_HIT3_HELD - FLOOR_EPS,
        "held file_hit@3 regressed below {MIN_FILE_HIT3_HELD}\n{table}"
    );

    insta::assert_snapshot!(table);
}
