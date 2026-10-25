//! Query surface over a compiled wiki: search, page lookup, neighbor packs.
//!
//! `leading_doc` takes only the first line of this comment as the page
//! summary, so that line has to stand on its own. Detail goes below it:
//! `neighbors` walks outward from a page and stops at a caller-supplied
//! token ceiling, degrading an oversized target to a title-and-summary block.

use crate::model::{CodeItem, CodeItemKind, KindFilter, SourceKind};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Deserialize)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) kind: String,
    pub(crate) aliases: Vec<String>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    /// Source path relative to the compile input.
    #[serde(default)]
    pub(crate) path: String,
    pub(crate) summary: Option<String>,
    #[serde(default)]
    pub(crate) updated: Option<String>,
    /// From an agent memory folder; absent in `index.json` when false.
    #[serde(default)]
    pub(crate) memory: bool,
    /// The id of the note that supersedes this one; absent in `index.json` when unset.
    #[serde(default)]
    pub(crate) superseded_by: Option<String>,
    pub(crate) pagerank: f64,
    pub(crate) token_estimate: usize,
    pub(crate) neighbors_out: Vec<String>,
    pub(crate) neighbors_in: Vec<String>,
    /// The import edges among `neighbors_out` and `neighbors_in`. Defaulted
    /// like `defined`: an older `index.json` has no such keys.
    #[serde(default)]
    pub(crate) imports_out: Vec<String>,
    #[serde(default)]
    pub(crate) imports_in: Vec<String>,
    /// Top-level definition names (`ManifestEntry::defined`), minus those
    /// only test items carry (dropped in `Wiki::load`). Defaulted so
    /// an `index.json` written by a compiler older than this field still
    /// loads: `serve` reads wikis it did not compile.
    #[serde(default)]
    pub(crate) defined: Vec<String>,
    /// Method names (`ManifestEntry::methods`), minus test-only ones like
    /// `defined`. Defaulted for the same reason as `defined`: an older
    /// `index.json` has no such key.
    #[serde(default)]
    pub(crate) methods: Vec<String>,
}

#[derive(Deserialize)]
struct IndexFile {
    /// `Manifest::root`; empty for an `index.json` older than the field.
    #[serde(default)]
    root: String,
    entries: Vec<Entry>,
}

/// The error for an id `resolve_id` could not place; `near` are its candidates.
pub(crate) fn unknown_page(arg: &str, near: &[String]) -> String {
    let closest = if near.is_empty() {
        String::new()
    } else {
        format!("; closest: {}", near.join(", "))
    };
    format!("unknown page {arg:?}{closest} — use search to find ids")
}

/// Normalizes a tag argument as the index stores tags: trimmed, no leading
/// `#` or trailing `/`, lowercase. `None` when nothing is left.
pub fn tag_filter(arg: &str) -> Option<String> {
    let t = arg.trim().trim_start_matches('#').trim();
    let t = t.trim_end_matches('/').to_lowercase();
    (!t.is_empty()).then_some(t)
}

/// `tag_filter` for a user argument, with the error text callers show.
pub fn tag_arg(arg: &str) -> Result<String, String> {
    tag_filter(arg).ok_or_else(|| format!("empty tag {arg:?}"))
}

/// Multiplies a memory page's final score, so a note outranks code that
/// matches the same words.
pub(crate) const MEMORY_BOOST: f64 = 4.0;

/// A calendar date written in a query; `year` is `None` when not given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QueryDate {
    year: Option<u32>,
    month: u32,
    day: u32,
}

/// Month names a query may use (lowercase), each with its month number:
/// Ukrainian and Russian nominative and genitive, English full and short.
const MONTH_NAMES: [(u32, &[&str]); 12] = [
    (
        1,
        &["січень", "січня", "январь", "января", "january", "jan"],
    ),
    (
        2,
        &["лютий", "лютого", "февраль", "февраля", "february", "feb"],
    ),
    (3, &["березень", "березня", "март", "марта", "march", "mar"]),
    (
        4,
        &["квітень", "квітня", "апрель", "апреля", "april", "apr"],
    ),
    (5, &["травень", "травня", "май", "мая", "may"]),
    (6, &["червень", "червня", "июнь", "июня", "june", "jun"]),
    (7, &["липень", "липня", "июль", "июля", "july", "jul"]),
    (
        8,
        &["серпень", "серпня", "август", "августа", "august", "aug"],
    ),
    (
        9,
        &[
            "вересень",
            "вересня",
            "сентябрь",
            "сентября",
            "september",
            "sep",
            "sept",
        ],
    ),
    (
        10,
        &["жовтень", "жовтня", "октябрь", "октября", "october", "oct"],
    ),
    (
        11,
        &[
            "листопад",
            "листопада",
            "ноябрь",
            "ноября",
            "november",
            "nov",
        ],
    ),
    (
        12,
        &["грудень", "грудня", "декабрь", "декабря", "december", "dec"],
    ),
];

impl QueryDate {
    /// A real calendar day; 29 February passes without a year.
    fn new(year: Option<u32>, month: u32, day: u32) -> Option<QueryDate> {
        let leap = year.is_none_or(|y| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0);
        let last = match month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            1..=12 => 31,
            _ => 0,
        };
        (1..=last)
            .contains(&day)
            .then_some(QueryDate { year, month, day })
    }

    /// Whether `updated` (`YYYY-MM-DD`) is this day.
    fn matches(&self, updated: &str) -> bool {
        let mut parts = updated.splitn(3, '-').map(|x| x.parse::<u32>().ok());
        let (Some(Some(year)), Some(Some(month)), Some(Some(day))) =
            (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        month == self.month && day == self.day && self.year.is_none_or(|y| y == year)
    }
}

/// `w` as a number of at most `max_len` ASCII digits.
fn number(w: Option<&String>, max_len: usize) -> Option<u32> {
    w.filter(|w| (1..=max_len).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|w| w.parse().ok())
}

/// `w` as a four-digit year.
fn year(w: Option<&String>) -> Option<u32> {
    w.filter(|w| w.len() == 4).and_then(|w| number(Some(w), 4))
}

/// The date one word writes alone: `YYYY-MM-DD`, `D.MM` or `D.MM.YYYY`.
fn numeric_date(word: &str) -> Option<QueryDate> {
    let parts: Vec<&str> = word.split(['.', '-']).collect();
    if !parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let num = |k: usize| parts.get(k).and_then(|p| p.parse::<u32>().ok());
    match (parts.as_slice(), word.contains('-')) {
        ([y, m, d], true) if [y.len(), m.len(), d.len()] == [4, 2, 2] => {
            QueryDate::new(num(0), num(1)?, num(2)?)
        }
        ([d, m], false) if m.len() == 2 && d.len() == 2 => QueryDate::new(None, num(1)?, num(0)?),
        ([d, m, y], false) if m.len() == 2 && d.len() == 2 && y.len() == 4 => {
            QueryDate::new(num(2), num(1)?, num(0)?)
        }
        _ => None,
    }
}

/// The dates a query writes, deduplicated, in query order: `YYYY-MM-DD`,
/// `D.MM[.YYYY]` (two-digit month), `<day> <month name> [year]` and
/// `<month name> <day>[,] [year]`. A month name or a number alone is none.
pub(crate) fn query_dates(query: &str) -> Vec<QueryDate> {
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .collect();
    let mut out: Vec<QueryDate> = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let found = if let Some((month, _)) = MONTH_NAMES
            .iter()
            .find(|(_, names)| names.contains(&word.as_str()))
        {
            let before = i.checked_sub(1).and_then(|j| number(words.get(j), 2));
            match (before, number(words.get(i + 1), 2)) {
                (Some(day), _) => QueryDate::new(year(words.get(i + 1)), *month, day),
                (None, Some(day)) => QueryDate::new(year(words.get(i + 2)), *month, day),
                _ => None,
            }
        } else {
            numeric_date(word)
        };
        if let Some(date) = found {
            if !out.contains(&date) {
                out.push(date);
            }
        }
    }
    out
}

/// Which pages a search sees: all, none of the memory notes, or only them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryScope {
    With,
    Without,
    Only,
}

pub struct Hit {
    pub id: String,
    pub title: String,
    /// The page's source path, relative to the compile input.
    pub path: String,
    pub summary: Option<String>,
    /// `YYYY-MM-DD` of the page's last change, when known.
    pub updated: Option<String>,
    pub score: f64,
    /// The page is an agent memory note; its score carries `MEMORY_BOOST`.
    pub memory: bool,
    /// The id of the note that supersedes this one; it gets no boost.
    pub superseded_by: Option<String>,
    /// Deterministic excerpt around the earliest body match (`None` for
    /// hits that matched only title, alias, defined name, summary, or a
    /// section heading — the summary explains those).
    pub snippet: Option<String>,
    /// The page's best-matching code items, best first; empty for docs pages
    /// and for code pages that matched without any item.
    pub symbols: Vec<SymbolLine>,
}

/// A line without its leading comment marker (`//!`, `///`, `//`, `/**`,
/// `/*`, `*/`, `*`, `#` and `##` headings), so a snippet reads as prose. A
/// marker counts only when whitespace or the line end follows it, so
/// `#include`, `#[derive]`, `*ptr` and `*note*` stay intact.
fn strip_comment_marker(line: &str) -> &str {
    let l = line.trim_start();
    let marker_end = |r: &str| r.is_empty() || r.starts_with(char::is_whitespace);
    for m in ["//!", "///", "//", "/**", "/*", "*/", "*"] {
        if let Some(r) = l.strip_prefix(m).filter(|r| marker_end(r)) {
            return r;
        }
    }
    match l.trim_start_matches('#') {
        r if r.len() < l.len() && marker_end(r) => r,
        _ => l,
    }
}

/// One code item shown under its page's hit.
pub struct SymbolLine {
    pub path: String,
    /// 1-based, inclusive, in the original file.
    pub start_line: usize,
    pub end_line: usize,
    /// `owner::name`, or `name` when the item has no owner.
    pub name: String,
    pub signature: String,
    /// `None` also when an earlier line of the same page shows the same doc.
    pub doc: Option<String>,
    pub test: bool,
}

/// The search fields a page earns from its items (`Wiki::symbol_fields`),
/// built once in `Wiki::load`.
#[derive(Default)]
struct SymbolFields {
    /// Whole item names, lowercased, each also as `owner::name`.
    whole: BTreeSet<String>,
    /// Words of non-method names, none of them in `whole`.
    words: BTreeSet<String>,
    /// Words of method names, none of them in `whole` or `words`.
    method_words: BTreeSet<String>,
    /// Words of the items' docs.
    doc_words: Vec<String>,
    /// Words of the items' whole docs, scored below `doc_words`.
    doc_rest_words: Vec<String>,
}

#[derive(Default)]
pub struct PackBudget {
    pub max_nodes: Option<usize>,
    pub max_tokens: Option<usize>,
    pub full_neighbors: bool,
    /// Only neighbors of this kind; the target is always shown.
    pub kind: Option<KindFilter>,
}

pub struct ContextPack {
    pub text: String,
    pub included: Vec<String>,
}

/// The phrase a degraded target block carries when a page's body does not
/// fit the pack's token budget. Tests assert on it; keep it in one place.
pub const OVER_BUDGET_NOTE: &str = "exceeds the budget";

/// What `Wiki::search` learns about one page in its first pass. Scores
/// need corpus-wide statistics (`df`, `avglen`), so per-page facts are
/// held until every page has been seen; the page text itself is not.
struct Candidate<'a> {
    entry: &'a Entry,
    /// Per query token, in query order: (summed field weights, body
    /// occurrences).
    per_token: Vec<(f64, usize)>,
    /// Whitespace-word count of the searchable content.
    len: usize,
    snippet: Option<String>,
}

/// `Default` is the empty wiki: what `serve --source` holds before its first compile.
#[derive(Default)]
pub struct Wiki {
    /// The compile input directory's name (`Manifest::root`), or empty.
    root: String,
    entries: BTreeMap<String, Entry>,
    /// Rendered Markdown per page, read once in `load` so a loaded `Wiki`
    /// is a snapshot: later changes to the files on disk do not reach it.
    bodies: BTreeMap<String, String>,
    /// Code items per page, read once from `items.json`; empty when that file
    /// is missing or invalid (older wikis).
    items: BTreeMap<String, Vec<CodeItem>>,
    /// Per page with items: the fields `symbol_fields` builds from them.
    symbol_fields: BTreeMap<String, SymbolFields>,
}

impl Wiki {
    /// Load a compiled output directory: reads `index.json` for metadata +
    /// adjacency and reads every page body from `<id>.md` once; a page whose
    /// file is missing has no body. `items.json` is read the same way: missing
    /// or invalid means no items.
    pub fn load(dir: &Path) -> Result<Wiki, std::io::Error> {
        let text = std::fs::read_to_string(dir.join("index.json"))?;
        let index: IndexFile = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut entries: BTreeMap<String, Entry> = index
            .entries
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();
        let bodies = entries
            .keys()
            .filter_map(|id| {
                let body = std::fs::read_to_string(dir.join(format!("{id}.md"))).ok()?;
                Some((id.clone(), body))
            })
            .collect();
        let items: BTreeMap<String, Vec<CodeItem>> =
            std::fs::read_to_string(dir.join("items.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
        // A `defined` or `methods` name that only test items carry earns its
        // page nothing: C++ and Go list test fns and mock methods there too.
        for e in entries.values_mut() {
            let Some(items) = items.get(&e.id) else {
                continue;
            };
            // `any` first: a name no item carries is kept.
            let test_only = |n: &String| {
                items.iter().any(|i| i.name == *n && i.test)
                    && items.iter().all(|i| i.name != *n || i.test)
            };
            e.defined.retain(|n| !test_only(n));
            e.methods.retain(|n| !test_only(n));
        }
        let symbol_fields = entries
            .values()
            .filter_map(|e| {
                let items = items.get(&e.id)?;
                let title = e.title.to_lowercase();
                let defined = Self::defined_terms(&title, &e.defined, &e.methods);
                let f = Self::symbol_fields(&title, &defined, items);
                Some((e.id.clone(), f))
            })
            .collect();
        Ok(Wiki {
            root: index.root,
            entries,
            bodies,
            items,
            symbol_fields,
        })
    }

    /// A page's code items, as compiled; empty when it has none.
    pub fn items(&self, id: &str) -> &[CodeItem] {
        self.items.get(id).map_or(&[], Vec::as_slice)
    }

    /// Every page with code items: its source path and its items.
    pub(crate) fn code_items(&self) -> impl Iterator<Item = (&str, &[CodeItem])> {
        self.items
            .iter()
            .filter_map(|(id, items)| Some((self.entries.get(id)?.path.as_str(), items.as_slice())))
    }

    /// A page's rendered Markdown body by id, as it was when loaded.
    pub fn page(&self, id: &str) -> Option<String> {
        self.bodies.get(id).cloned()
    }

    /// A page's body without cloning it; empty when the id is unknown.
    fn body(&self, id: &str) -> &str {
        self.bodies.get(id).map_or("", String::as_str)
    }

    /// A page's index entry by exact id.
    pub(crate) fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.get(id)
    }

    /// Whether `id` is a page in the loaded index.
    pub fn has_page(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    /// All pages as `(id, title)` pairs, ascending by id (`BTreeMap` order).
    /// Used by the MCP server's `resources/list`.
    pub fn list_pages(&self) -> Vec<(String, String)> {
        self.entries
            .values()
            .map(|e| (e.id.clone(), e.title.clone()))
            .collect()
    }

    /// Sections excluded from search: generated chrome, so a query like
    /// "related" or "metadata" does not false-positive on every page.
    ///
    /// Both this list and `GENERATED_HEADINGS` are compared exact-case, on
    /// purpose: `rewrite.rs` emits the chrome with this exact casing, so the
    /// exact match excludes the generated sections and nothing else. A
    /// case-insensitive match would also hide an author's own `## notes` or
    /// `## NOTES` section (2026-09-02 audit, findings-log item 37).
    /// `Referenced By` is what `Imported by` and `Mentioned in` replaced: a
    /// served wiki may be older than the compiler.
    const CHROME_SECTIONS: [&'static str; 6] = [
        "Metadata",
        "Related",
        "Imported by",
        "Mentioned in",
        "Referenced By",
        "Notes",
    ];

    /// Generated content headings excluded from the `heading` field: they
    /// sit on nearly every page and say nothing about it. Measured on the
    /// eval corpus, leaving them in made `exports` hit 17 of 17 pages.
    const GENERATED_HEADINGS: [&'static str; 3] = ["Body", "Exports", "Imports"];

    // Field weights for search scoring. Values from the 2026-09-02
    // search-scoring design; tuning is a constants-only change.
    // `W_HEADING` is deliberately below `W_SUMMARY`: at 1.5 an incidental
    // heading word on a large page outranked the page named for the query.
    const W_NAME: f64 = 3.0;
    // Added per token when the query is the page's whole title: a title
    // query finds its page although symbol words of other pages share the
    // title's words (`summary_suffix` against the page `Summary`).
    const W_TITLE: f64 = 3.0;
    const W_ALIAS: f64 = 2.0;
    // `W_DEFINED` (2026-09-02 defined-names design): equal to `W_ALIAS`,
    // above `W_SUMMARY`, below `W_NAME`, so a page whose title is the query
    // still beats a page that merely defines a same-named item; `W_SYMBOL`,
    // which replaces it for an item's whole name, stays below `W_NAME` too.
    // Measured on the 547-query live set: own-title hits unchanged at 35/37,
    // 26 flips better / 6 worse; 1.5 moved fewer definer queries, 2.5 added
    // a worse row.
    const W_DEFINED: f64 = 2.0;
    // Page fields from code items (`symbol_fields`), private and method
    // items included, test and module items never. A whole item name
    // scores `W_SYMBOL` in place of `W_DEFINED`, above every word weight
    // plus the doc weight and below `W_NAME` (both checked at compile time
    // below), so a page naming the query beats a page whose names merely
    // contain it, and a page titled by the query beats both. Tuned on the
    // symbol eval's `tune` split (2026-10-23): `W_SYMBOL_DOC` 0.5 → 0.75
    // moved a doc query to file@1; 1.0 cost a title query. `W_SYMBOL` 3.0 →
    // 2.8 restored the `W_NAME` rule and moved no eval rank.
    const W_SYMBOL: f64 = 2.8;
    const W_SYMBOL_WORD: f64 = 2.0;
    const W_METHOD_WORD: f64 = 2.0;
    const W_SYMBOL_DOC: f64 = 0.75;
    // A token the first sentence misses but the rest of the doc has.
    const W_SYMBOL_DOC_REST: f64 = Self::W_SYMBOL_DOC / 2.0;
    const W_SUMMARY: f64 = 1.5;
    const W_HEADING: f64 = 1.0;
    const W_BODY: f64 = 1.0;
    // BM25 term-frequency saturation (`K1`) and length-normalisation
    // strength (`B`). Textbook values, untuned: nineteen eval cases cannot
    // tell `B = 0.5` from `0.75`.
    const K1: f64 = 1.2;
    const B: f64 = 0.75;

    /// The searchable *content* of a parsed page: every section body except
    /// the generated chrome (`CHROME_SECTIONS`). Subtractive on purpose — a
    /// doc body's own `## ` subheadings become sections of their own in
    /// `parse_sections`, and their text must stay searchable. The heading
    /// names themselves are scored separately (see `search`).
    ///
    /// Known residual limits (accepted by the 2026-07-14 search-quality
    /// design): content under an embedded heading named exactly like a
    /// chrome section stays unsearchable, and duplicate heading names
    /// overwrite each other in the map. Section order is `BTreeMap`
    /// (alphabetical), not document, order.
    fn content_text(sections: &BTreeMap<String, String>) -> String {
        sections
            .iter()
            .filter(|(k, _)| !Self::CHROME_SECTIONS.contains(&k.as_str()))
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Query tokenization for search: lowercase, split on whitespace, trim
    /// each piece's leading/trailing non-alphanumeric chars (interior chars
    /// like `_` and `:` survive), drop empties, dedupe keeping first order.
    fn tokenize(query: &str) -> Vec<String> {
        let mut tokens: Vec<String> = Vec::new();
        for raw in query.to_lowercase().split_whitespace() {
            let t = raw.trim_matches(|c: char| !c.is_alphanumeric());
            if !t.is_empty() && !tokens.iter().any(|x| x == t) {
                tokens.push(t.to_string());
            }
        }
        tokens
    }

    /// Cut a Cyrillic grammatical ending (longest first) when at least four
    /// letters remain, so another form of a word still matches. Other tokens
    /// are returned unchanged.
    fn stem(token: &str) -> String {
        const ENDINGS: [&str; 38] = [
            "ами", "ями", "ові", "еві", "ого", "ому", "ими", "іми", "ій", "ий", "ою", "ею", "ів",
            "їв", "ах", "ях", "ам", "ям", "ом", "ем", "ої", "а", "я", "і", "ї", "и", "у", "ю", "о",
            "е", "є", "ь", "ити", "ати", "ють", "ують", "ться", "ся",
        ];
        if !token.chars().any(|c| ('\u{400}'..='\u{4FF}').contains(&c)) {
            return token.to_string();
        }
        let len = token.chars().count();
        ENDINGS
            .iter()
            .filter(|e| token.ends_with(**e) && len - e.chars().count() >= 4)
            .max_by_key(|e| e.chars().count())
            .map_or_else(
                || token.to_string(),
                |e| token[..token.len() - e.len()].to_string(),
            )
    }

    /// `tokenize`, then `stem` each word and dedupe again: (stemmed, raw).
    fn query_tokens(query: &str) -> (Vec<String>, Vec<String>) {
        let raw = Self::tokenize(query);
        let mut stemmed: Vec<String> = Vec::new();
        for t in &raw {
            let s = Self::stem(t);
            if !stemmed.contains(&s) {
                stemmed.push(s);
            }
        }
        (stemmed, raw)
    }

    /// The words of an identifier, lowercased: split on `_`/`-`, at a
    /// lower→Upper boundary, and at an Upper→Upper-lower boundary so an
    /// acronym run stays one word (`HTTPServer` → `http`, `server`;
    /// `PackBudget` → `pack`, `budget`; `CACHE_VERSION` → `cache`,
    /// `version`). Must run on the ORIGINAL-case name: lowercasing first
    /// erases every CamelCase boundary (the 2026-09-02 V7 prototype's dead
    /// rule).
    pub(crate) fn name_words(name: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut cur = String::new();
        let chars: Vec<char> = name.chars().collect();
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

    /// The words of a lowercased field: maximal runs of alphanumeric
    /// characters (`char::is_alphanumeric`, so digits count and `_`, `-`,
    /// `:` and punctuation split). Computed once per page per field.
    fn field_words(field: &str) -> Vec<String> {
        field
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Whether query token `t` hits a field given its `field_words`: every
    /// alphanumeric part of `t` is a prefix of some word of the field
    /// (prefix includes equality; parts may match non-adjacent words).
    /// Replaces substring matching, under which `write` hit the title
    /// `Rewrite` and `id` hit `process-wide`; a prefix still catches the
    /// derived forms substring caught (`wikilink` → `wikilinks`, `dir` →
    /// `directory`). `tokenize` trims edges and drops empties with this
    /// same predicate, so a part-less token cannot arrive and the vacuous
    /// `all` is safe.
    fn field_hit(words: &[String], t: &str) -> bool {
        debug_assert!(
            t.chars().any(char::is_alphanumeric),
            "tokenize never yields a token without an alphanumeric part"
        );
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|p| !p.is_empty())
            .all(|p| words.iter().any(|w| w.starts_with(p)))
    }

    /// Whether any query token of `q` (stemmed, as `search` reads it) hits
    /// the title, an alias or the summary of page `id` by `field_hit`'s
    /// prefix rule; tokens under `min_len` characters are skipped (`в`, `що`,
    /// `the` hit nearly every note). The body is not looked at: a hint gates
    /// on this, because a Ukrainian prompt matches nearly every Ukrainian
    /// note body.
    pub(crate) fn names_page(&self, id: &str, q: &str, min_len: usize) -> bool {
        let Some(e) = self.entries.get(id) else {
            return false;
        };
        let fields = std::iter::once(e.title.as_str())
            .chain(e.aliases.iter().map(String::as_str))
            .chain(e.summary.as_deref())
            .map(|f| Self::field_words(&f.to_lowercase()))
            .collect::<Vec<_>>();
        Self::query_tokens(q)
            .0
            .iter()
            .filter(|t| t.chars().count() >= min_len)
            .any(|t| fields.iter().any(|w| Self::field_hit(w, t)))
    }

    /// Whether `q` writes a date and page `id` is a memory note dated that day.
    pub(crate) fn dated_on(&self, id: &str, q: &str) -> bool {
        let Some(e) = self.entries.get(id).filter(|e| e.memory) else {
            return false;
        };
        let dates = query_dates(q);
        e.updated
            .as_deref()
            .is_some_and(|u| dates.iter().any(|d| d.matches(u)))
    }

    /// The search terms a page earns from its `defined` names — each
    /// lowercased full name plus each of its words, minus the self-name
    /// skip — plus every `methods` name lowercased whole, minus names equal
    /// to a title word. The skip is word-level: a word equal to a title
    /// word is dropped (the title already scores it at `W_NAME`; crediting
    /// it again let `Cache` on page `cache` outrank `code`), the other
    /// words stay, and the full name stays unless it is itself a title
    /// word — the title never matches the full-name token, so dropping the
    /// whole name would leave the definer with no credit (`codeextractor`
    /// on `code`).
    fn defined_terms(
        title_lower: &str,
        defined: &[String],
        methods: &[String],
    ) -> BTreeSet<String> {
        let title_words: Vec<&str> = title_lower.split_whitespace().collect();
        let mut terms = BTreeSet::new();
        for name in defined {
            let full = name.to_lowercase();
            if !title_words.contains(&full.as_str()) {
                terms.insert(full);
            }
            terms.extend(
                Self::name_words(name)
                    .into_iter()
                    .filter(|w| !title_words.contains(&w.as_str())),
            );
        }
        // Method names whole, never through `name_words`: splitting them
        // re-creates the V7 failure (`needs_render` credits `render`).
        for name in methods {
            let full = name.to_lowercase();
            if !title_words.contains(&full.as_str()) {
                terms.insert(full);
            }
        }
        terms
    }

    /// The fields a page earns from its non-test, non-module items. Whole
    /// names lose the page's own title words; an owned item's whole name
    /// also enters as `owner::name`. Words also lose terms already
    /// in its `defined_terms`, and each word lands in one field only: a
    /// whole name, else a free name's word, else a method's word. Another
    /// page's title words stay: dropping them hid `onOrderReject` from
    /// `order reject` on a 2,700-page wiki; `W_TITLE` guards title queries.
    fn symbol_fields(
        title_lower: &str,
        defined_terms: &BTreeSet<String>,
        items: &[CodeItem],
    ) -> SymbolFields {
        let title_words: Vec<&str> = title_lower.split_whitespace().collect();
        let keep_word =
            |w: &String| !title_words.contains(&w.as_str()) && !defined_terms.contains(w);
        let mut f = SymbolFields::default();
        for item in items
            .iter()
            .filter(|i| !i.test && i.kind != CodeItemKind::Module)
        {
            let whole = item.name.to_lowercase();
            if !title_words.contains(&whole.as_str()) {
                f.whole.insert(whole);
            }
            if item.owner.is_some() {
                f.whole.insert(item.qualified_name().to_lowercase());
            }
            let words = Self::name_words(&item.name).into_iter().filter(keep_word);
            if item.kind == CodeItemKind::Method {
                f.method_words.extend(words);
            } else {
                f.words.extend(words);
            }
            if let Some(doc) = &item.doc {
                f.doc_words.extend(Self::field_words(&doc.to_lowercase()));
            }
            if let Some(text) = &item.doc_text {
                f.doc_rest_words
                    .extend(Self::field_words(&text.to_lowercase()));
            }
        }
        f.words.retain(|w| !f.whole.contains(w));
        f.method_words
            .retain(|w| !f.whole.contains(w) && !f.words.contains(w));
        f
    }

    // Item scores for the symbol lines under a hit: per query token,
    // `idf · (whole name or name word) + idf · doc`, test items scaled down.
    // A whole-name match outranks any word match, even one whose item also
    // matches by doc (checked at compile time below): at 2.5, graph.rs
    // `resolve_link`'s word plus its doc put it above the method `resolve`.
    const IW_WHOLE: f64 = 3.5;
    const IW_NAME_WORD: f64 = 2.0;
    const IW_METHOD_WORD: f64 = 2.0;
    const IW_DOC: f64 = 1.0;
    const IW_DOC_REST: f64 = Self::IW_DOC / 2.0;
    const TEST_ITEM_FACTOR: f64 = 0.5;
    /// Symbol lines per page at most.
    const SYMBOLS_PER_PAGE: usize = 3;
    /// A line is shown only if it scores at least this share of the best line
    /// in the whole response.
    const MIN_REL_SYMBOL: f64 = 0.3;

    fn item_score(item: &CodeItem, tokens: &[String], idf: &[f64]) -> f64 {
        let whole = item.name.to_lowercase();
        let qualified = item.qualified_name().to_lowercase();
        let words = Self::name_words(&item.name);
        let w_word = if item.kind == CodeItemKind::Method {
            Self::IW_METHOD_WORD
        } else {
            Self::IW_NAME_WORD
        };
        let doc = item
            .doc
            .as_deref()
            .map(|d| Self::field_words(&d.to_lowercase()))
            .unwrap_or_default();
        let doc_rest = item
            .doc_text
            .as_deref()
            .map(|d| Self::field_words(&d.to_lowercase()))
            .unwrap_or_default();
        let score: f64 = tokens
            .iter()
            .zip(idf)
            .map(|(t, &idf_t)| {
                let name = if *t == whole || *t == qualified {
                    Self::IW_WHOLE
                } else if words.contains(t) {
                    w_word
                } else {
                    0.0
                };
                let doc = if !doc.is_empty() && Self::field_hit(&doc, t) {
                    Self::IW_DOC
                } else if !doc_rest.is_empty() && Self::field_hit(&doc_rest, t) {
                    Self::IW_DOC_REST
                } else {
                    0.0
                };
                idf_t * (name + doc)
            })
            .sum();
        if item.test {
            score * Self::TEST_ITEM_FACTOR
        } else {
            score
        }
    }

    /// A code page's best items for `tokens`, best first (ties by line),
    /// one per `(start, end, owner::name)`, at most `SYMBOLS_PER_PAGE`.
    fn page_symbols<'a>(
        &'a self,
        id: &str,
        tokens: &[String],
        idf: &[f64],
    ) -> Vec<(f64, &'a CodeItem)> {
        let mut scored: Vec<(f64, &CodeItem)> = self
            .items(id)
            .iter()
            .filter(|i| i.kind != CodeItemKind::Module)
            .map(|i| (Self::item_score(i, tokens, idf), i))
            .filter(|(s, _)| *s > 0.0)
            .collect();
        scored.sort_by(|(a, x), (b, y)| {
            b.total_cmp(a)
                .then_with(|| x.start_line.cmp(&y.start_line))
                .then_with(|| x.name.cmp(&y.name))
        });
        let mut seen = BTreeSet::new();
        scored.retain(|(_, i)| seen.insert((i.start_line, i.end_line, &i.owner, &i.name)));
        scored.truncate(Self::SYMBOLS_PER_PAGE);
        scored
    }

    const SNIPPET_CONTEXT_CHARS: usize = 60;

    /// Excerpt around the earliest occurrence of any query token in the
    /// content: 60 chars of context each side, whitespace runs collapsed,
    /// `…` on truncated edges. Match offsets come from the lowercased text;
    /// on the rare non-ASCII page where lowercasing changes byte lengths the
    /// window may sit slightly off, but boundary snapping guarantees it
    /// never panics and stays deterministic.
    fn snippet(content: &str, content_lower: &str, tokens: &[String]) -> Option<String> {
        // Earliest occurrence of any token; ties at the same index go to the
        // longer token.
        let mut best: Option<(usize, usize)> = None;
        for t in tokens {
            if let Some(i) = content_lower.find(t.as_str()) {
                best = match best {
                    Some((bi, bl)) if bi < i || (bi == i && bl >= t.len()) => Some((bi, bl)),
                    _ => Some((i, t.len())),
                };
            }
        }
        let (m_start, m_len) = best?;

        // Clamp into the original string, then snap inward to char
        // boundaries (lowercasing can shift byte offsets on non-ASCII).
        let mut start = m_start.min(content.len());
        while !content.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = (m_start + m_len).min(content.len());
        while !content.is_char_boundary(end) {
            end += 1;
        }

        // Widen by up to SNIPPET_CONTEXT_CHARS chars on each side.
        let before: usize = content[..start]
            .chars()
            .rev()
            .take(Self::SNIPPET_CONTEXT_CHARS)
            .map(char::len_utf8)
            .sum();
        let after: usize = content[end..]
            .chars()
            .take(Self::SNIPPET_CONTEXT_CHARS)
            .map(char::len_utf8)
            .sum();
        let (w_start, w_end) = (start - before, end + after);

        let mut s = content[w_start..w_end]
            .lines()
            .flat_map(|l| strip_comment_marker(l).split_whitespace())
            .collect::<Vec<_>>()
            .join(" ");
        if w_start > 0 {
            s.insert(0, '…');
        }
        if w_end < content.len() {
            s.push('…');
        }
        Some(s)
    }

    /// Pass 1 of `search` for one page: its searchable word count (always,
    /// for `avglen`) and, when at least one token hits any field, the
    /// per-token facts scored in pass 2. Bumps `df[i]` for every token `i`
    /// that hits.
    fn candidate<'a>(
        &self,
        e: &'a Entry,
        tokens: &[String],
        raw: &[String],
        dates: &[QueryDate],
        df: &mut [usize],
    ) -> (usize, Option<Candidate<'a>>) {
        let title = e.title.to_lowercase();
        let title_words = Self::field_words(&title);
        // The query is this page's whole title, in any order.
        let whole_title = raw
            .iter()
            .flat_map(|t| Self::field_words(t))
            .collect::<BTreeSet<_>>()
            == title_words.iter().cloned().collect();
        let alias_words: Vec<Vec<String>> = e
            .aliases
            .iter()
            .map(|a| Self::field_words(&a.to_lowercase()))
            .collect();
        let summary_words: Option<Vec<String>> = e
            .summary
            .as_deref()
            .map(|s| Self::field_words(&s.to_lowercase()));
        let defined_terms = Self::defined_terms(&title, &e.defined, &e.methods);
        let symbols = self.symbol_fields.get(&e.id);
        let sections = crate::rewrite::parse_sections(self.body(&e.id));
        let headings: Vec<String> = sections
            .keys()
            .filter(|k| {
                !Self::CHROME_SECTIONS.contains(&k.as_str())
                    && !Self::GENERATED_HEADINGS.contains(&k.as_str())
            })
            .map(|k| k.to_lowercase())
            .collect();
        let heading_words: Vec<Vec<String>> =
            headings.iter().map(|h| Self::field_words(h)).collect();
        let content = Self::content_text(&sections);
        let content_lower = content.to_lowercase();
        let len = content.split_whitespace().count();

        let mut per_token = Vec::with_capacity(tokens.len());
        let mut matched = false;
        let mut any_body = false;
        for (i, t) in tokens.iter().enumerate() {
            let t = t.as_str();
            let mut weight = 0.0;
            if Self::field_hit(&title_words, t) {
                weight += Self::W_NAME;
            }
            if whole_title {
                weight += Self::W_TITLE;
            }
            if alias_words.iter().any(|w| Self::field_hit(w, t)) {
                weight += Self::W_ALIAS;
            }
            // Whole-term equality, never `contains`: `to` must not hit
            // `token_estimate`, `wiki` must not hit `WikiError`.
            // A whole item name scores `W_SYMBOL` instead of `W_DEFINED`.
            if symbols.is_some_and(|f| f.whole.contains(t)) {
                weight += Self::W_SYMBOL;
            } else if defined_terms.contains(t) {
                weight += Self::W_DEFINED;
            }
            if let Some(f) = symbols {
                if f.words.contains(t) {
                    weight += Self::W_SYMBOL_WORD;
                }
                if f.method_words.contains(t) {
                    weight += Self::W_METHOD_WORD;
                }
                if Self::field_hit(&f.doc_words, t) {
                    weight += Self::W_SYMBOL_DOC;
                } else if Self::field_hit(&f.doc_rest_words, t) {
                    weight += Self::W_SYMBOL_DOC_REST;
                }
            }
            if summary_words
                .as_deref()
                .is_some_and(|w| Self::field_hit(w, t))
            {
                weight += Self::W_SUMMARY;
            }
            if heading_words.iter().any(|w| Self::field_hit(w, t)) {
                weight += Self::W_HEADING;
            }
            // match_indices is non-overlapping — the spec'd counting rule.
            // `tf` may exceed `len` for short tokens ("on" in "one",
            // "long"); `tf'` saturates, so that is harmless.
            let tf = content_lower.match_indices(t).count();
            any_body |= tf > 0;
            if weight > 0.0 || tf > 0 {
                matched = true;
                df[i] += 1;
            }
            per_token.push((weight, tf));
        }
        // One synthetic token per query date: a memory note dated that day.
        for (k, date) in dates.iter().enumerate() {
            let hit = e.memory && e.updated.as_deref().is_some_and(|u| date.matches(u));
            if hit {
                matched = true;
                df[tokens.len() + k] += 1;
            }
            per_token.push((if hit { Self::W_TITLE } else { 0.0 }, 0));
        }
        if !matched {
            return (len, None);
        }
        let snippet = if any_body {
            Self::snippet(&content, &content_lower, tokens)
        } else {
            None
        };
        let candidate = Candidate {
            entry: e,
            per_token,
            len,
            snippet,
        };
        (len, Some(candidate))
    }

    /// Case-insensitive tokenized search over name/alias/defined names/summary/section headings/body.
    /// Partial matching: a page is a hit if any token matches
    /// any field.
    /// Name, alias, summary and heading match when every alphanumeric part of the token is a prefix of one of the field's words (`field_hit`); body by substring; defined names by whole term (the lowercased name or one of its words, see `defined_terms`).
    ///
    /// Two passes. Pass 1 collects per-page facts and corpus statistics;
    /// pass 2 scores. Per token `t`:
    ///
    /// ```text
    /// idf(t) = ln(1 + (N − df(t) + 0.5) / (df(t) + 0.5))        > 0 always
    /// tf'    = tf·(K1+1) / (tf + K1·(1 − B + B·len/avglen))      ≤ K1+1
    /// score  = Σ_t idf(t) · (field weights hit by t + W_BODY·tf')
    /// ```
    ///
    /// `N`, `df`, `avglen` are taken over the pages that pass `kind`, so a
    /// page's `score` differs between filtered and unfiltered calls: it is a
    /// ranking key, not a stable property. Field weights are not length-
    /// normalised, and `tf'` saturates below `W_NAME`, so a title hit beats
    /// any volume of body text on the same token. Sorted by score desc,
    /// then pagerank desc, then id asc; truncated to `limit`.
    /// Empty/punctuation-only queries return no hits.
    ///
    /// Code pages also score their items' names, method words and docs
    /// (`symbol_fields`, weights `W_SYMBOL`, `W_SYMBOL_WORD`, `W_METHOD_WORD`,
    /// `W_SYMBOL_DOC`),
    /// and each returned code page carries its best items as `symbols`
    /// (`expand`), with the same `idf`.
    pub fn search(&self, q: &str, kind: Option<&KindFilter>, limit: usize) -> Vec<Hit> {
        self.search_tagged(q, kind, None, Some(limit))
    }

    /// `search` narrowed to pages tagged `tag` (already normalized by
    /// `tag_filter`) or a sub-tag of it; the tag check sits beside `kind`, so
    /// it narrows the corpus statistics too. With no query tokens and a tag,
    /// lists every page that passes both filters by path, score 0 — `limit`
    /// truncates only when given; with a query the default is 10.
    pub fn search_tagged(
        &self,
        q: &str,
        kind: Option<&KindFilter>,
        tag: Option<&str>,
        limit: Option<usize>,
    ) -> Vec<Hit> {
        self.search_scoped(q, kind, tag, limit, MemoryScope::With)
    }

    /// `search_tagged` over the memory notes only, all but them, or both.
    /// A memory note passes the `code` and `docs` groups whatever its real
    /// kind (agents filter by `code`); an exact kind matches its real kind.
    /// Memory hits' scores are multiplied by `MEMORY_BOOST` (a superseded
    /// note gets neither the boost nor the tie keys), and of two
    /// equal-scored notes the later `updated` comes first (dated before
    /// undated).
    pub(crate) fn search_scoped(
        &self,
        q: &str,
        kind: Option<&KindFilter>,
        tag: Option<&str>,
        limit: Option<usize>,
        scope: MemoryScope,
    ) -> Vec<Hit> {
        let (tokens, raw) = Self::query_tokens(q);
        let dates = query_dates(q);
        let passes = |e: &Entry| {
            (match scope {
                MemoryScope::With => true,
                MemoryScope::Without => !e.memory,
                MemoryScope::Only => e.memory,
            }) && kind.is_none_or(|f| {
                (e.memory && matches!(f, KindFilter::Code | KindFilter::Docs))
                    || SourceKind::parse(&e.kind).is_some_and(|k| f.matches(&k))
            }) && tag.is_none_or(|t| {
                e.tags
                    .iter()
                    .any(|g| g == t || g.strip_prefix(t).is_some_and(|r| r.starts_with('/')))
            })
        };
        if tokens.is_empty() {
            let Some(_) = tag else {
                return Vec::new();
            };
            let mut listed: Vec<&Entry> = self.entries.values().filter(|e| passes(e)).collect();
            listed.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.id.cmp(&b.id)));
            listed.truncate(limit.unwrap_or(usize::MAX));
            return listed
                .into_iter()
                .map(|e| Hit {
                    id: e.id.clone(),
                    title: e.title.clone(),
                    path: e.path.clone(),
                    summary: e.summary.clone(),
                    updated: e.updated.clone(),
                    score: 0.0,
                    memory: e.memory,
                    superseded_by: e.superseded_by.clone(),
                    snippet: None,
                    symbols: Vec::new(),
                })
                .collect();
        }
        let limit = limit.unwrap_or(10);

        // Pass 1: facts per page, statistics over every filtered page.
        let mut n = 0usize;
        let mut total_len = 0usize;
        let mut df = vec![0usize; tokens.len() + dates.len()];
        let mut candidates: Vec<Candidate> = Vec::new();
        for e in self.entries.values() {
            if !passes(e) {
                continue;
            }
            let (len, candidate) = self.candidate(e, &tokens, &raw, &dates, &mut df);
            // Statistics count every filtered page, hits or not — `avglen`
            // over hits alone would change with the query.
            n += 1;
            total_len += len;
            candidates.extend(candidate);
        }
        if candidates.is_empty() {
            return Vec::new();
        }

        // Pass 2: score. `n > 0` here because a candidate was counted.
        let n_f = n as f64;
        let avglen = total_len as f64 / n_f;
        // `avglen == 0` needs every filtered page's body to be missing on
        // disk; guard once here rather than divide by zero per page.
        let avglen = if avglen > 0.0 { avglen } else { 1.0 };
        let idf: Vec<f64> = df
            .iter()
            .map(|&d| (1.0 + (n_f - d as f64 + 0.5) / (d as f64 + 0.5)).ln())
            .collect();
        let mut ranked: Vec<(Hit, f64)> = candidates
            .into_iter()
            .map(|c| {
                let norm = c.len as f64 / avglen;
                let base: f64 = c
                    .per_token
                    .iter()
                    .zip(&idf)
                    .map(|(&(weight, tf), &idf_t)| {
                        let tf = tf as f64;
                        let tf_norm = tf * (Self::K1 + 1.0)
                            / (tf + Self::K1 * (1.0 - Self::B + Self::B * norm));
                        idf_t * (weight + Self::W_BODY * tf_norm)
                    })
                    .sum();
                let total = if c.entry.memory && c.entry.superseded_by.is_none() {
                    base * MEMORY_BOOST
                } else {
                    base
                };
                let hit = Hit {
                    id: c.entry.id.clone(),
                    title: c.entry.title.clone(),
                    path: c.entry.path.clone(),
                    summary: c.entry.summary.clone(),
                    updated: c.entry.updated.clone(),
                    score: total,
                    memory: c.entry.memory,
                    superseded_by: c.entry.superseded_by.clone(),
                    snippet: c.snippet,
                    symbols: Vec::new(),
                };
                (hit, c.entry.pagerank)
            })
            .collect();
        // Only live notes (not superseded) take the memory tie keys.
        let live = |h: &Hit| h.memory && h.superseded_by.is_none();
        ranked.sort_by(|(a, a_pr), (b, b_pr)| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| live(b).cmp(&live(a)))
                .then_with(|| {
                    if live(a) {
                        b.updated.cmp(&a.updated)
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then_with(|| b_pr.total_cmp(a_pr))
                .then_with(|| a.id.cmp(&b.id))
        });
        ranked.truncate(limit);
        let mut hits: Vec<Hit> = ranked.into_iter().map(|(hit, _)| hit).collect();
        self.expand(&mut hits, &tokens, &idf[..tokens.len()]);
        hits
    }

    /// Gives each code hit its symbol lines (`page_symbols`), keeping only
    /// lines at or above `MIN_REL_SYMBOL` of the response's best line.
    fn expand(&self, hits: &mut [Hit], tokens: &[String], idf: &[f64]) {
        let per_hit: Vec<Vec<(f64, &CodeItem)>> = hits
            .iter()
            .map(|h| {
                let code = self.entries.get(&h.id).is_some_and(|e| {
                    SourceKind::parse(&e.kind).is_some_and(|k| KindFilter::Code.matches(&k))
                });
                if code {
                    self.page_symbols(&h.id, tokens, idf)
                } else {
                    Vec::new()
                }
            })
            .collect();
        let best = per_hit
            .iter()
            .flatten()
            .map(|(s, _)| *s)
            .fold(0.0, f64::max);
        for (hit, lines) in hits.iter_mut().zip(per_hit) {
            let mut docs = BTreeSet::new();
            hit.symbols = lines
                .into_iter()
                .filter(|(s, _)| *s >= Self::MIN_REL_SYMBOL * best)
                .map(|(_, i)| SymbolLine {
                    path: hit.path.clone(),
                    start_line: i.start_line,
                    end_line: i.end_line,
                    name: i.qualified_name(),
                    signature: i.signature.clone(),
                    doc: i.doc.clone().filter(|d| docs.insert(d.clone())),
                    test: i.test,
                })
                .collect();
        }
    }

    /// Ids within `depth` hops of `id` along either edge direction,
    /// including `id` itself.
    fn bfs_neighborhood(&self, id: &str, depth: usize) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        seen.insert(id.to_string());
        let mut frontier = vec![id.to_string()];
        for _ in 0..depth {
            let mut next = Vec::new();
            for nid in &frontier {
                if let Some(e) = self.entries.get(nid) {
                    for n in e.neighbors_out.iter().chain(e.neighbors_in.iter()) {
                        if seen.insert(n.clone()) {
                            next.push(n.clone());
                        }
                    }
                }
            }
            frontier = next;
        }
        seen
    }

    /// Between the blocks of a context pack.
    pub(crate) const SEPARATOR: &str = "\n\n---\n\n";

    /// Pagerank of `id`, or 0 for an id the index does not know.
    fn pagerank_of(&self, id: &str) -> f64 {
        self.entries.get(id).map_or(0.0, |e| e.pagerank)
    }

    /// `resolve_id` with the unknown-page error text callers show.
    pub fn resolve(&self, arg: &str) -> Result<String, String> {
        self.resolve_id(arg)
            .map_err(|near| unknown_page(arg, &near))
    }

    /// Resolves what a caller typed to a page id: an exact id, a title
    /// (ignoring case), the slug of `arg`, then `arg` as a path (`\` read as
    /// `/`, one leading `./` dropped): the page whose source path is it, the
    /// one page whose path ends with `/arg`, or the page with the longest path
    /// that `arg` ends with as `/{root}/{path}` (an absolute path under the
    /// compiled directory; none when the wiki has no `root`). Otherwise up to
    /// 5 candidate ids, sorted: the several pages that suffix matched, else
    /// ids sharing a lowercase word with `arg`.
    pub fn resolve_id(&self, arg: &str) -> Result<String, Vec<String>> {
        if self.entries.contains_key(arg) {
            return Ok(arg.to_string());
        }
        let by = |f: &dyn Fn(&Entry) -> bool| {
            let mut m = self.entries.values().filter(|e| f(e));
            match (m.next(), m.next()) {
                (Some(e), None) => Some(e.id.clone()),
                _ => None,
            }
        };
        if let Some(id) = by(&|e| e.title.to_lowercase() == arg.to_lowercase()) {
            return Ok(id);
        }
        let slug = crate::model::slugify(arg);
        if self.entries.contains_key(&slug) {
            return Ok(slug);
        }
        let norm = arg.replace('\\', "/");
        let norm = norm.strip_prefix("./").unwrap_or(&norm);
        if let Some(id) = by(&|e| e.path == norm) {
            return Ok(id);
        }
        let suffix = format!("/{norm}");
        let by_path: Vec<String> = self
            .entries
            .values()
            .filter(|e| e.path.ends_with(&suffix))
            .map(|e| e.id.clone())
            .collect();
        if let [id] = by_path.as_slice() {
            return Ok(id.clone());
        }
        if !by_path.is_empty() {
            return Err(by_path.into_iter().take(5).collect());
        }
        // A longer path such as an absolute one, `…/{root}/{path}`: the
        // longest such page path. Without a root (an older wiki) nothing is.
        if let Some(e) = self
            .entries
            .values()
            .filter(|e| !self.root.is_empty() && !e.path.is_empty())
            .filter(|e| norm.ends_with(&format!("/{}/{}", self.root, e.path)))
            .max_by_key(|e| e.path.len())
        {
            return Ok(e.id.clone());
        }
        let words = |s: &str| -> BTreeSet<String> {
            s.split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_lowercase)
                .collect()
        };
        let want = words(arg);
        // BTreeMap iteration is already sorted by id.
        Err(self
            .entries
            .values()
            .filter(|e| !want.is_disjoint(&words(&e.id)) || !want.is_disjoint(&words(&e.title)))
            .map(|e| e.id.clone())
            .take(5)
            .collect())
    }

    /// Whether page `id` is a code page.
    fn is_code(&self, id: &str) -> bool {
        self.entries
            .get(id)
            .and_then(|e| SourceKind::parse(&e.kind))
            .is_some_and(|k| KindFilter::Code.matches(&k))
    }

    /// BFS to `depth` over `neighbors_out+neighbors_in`, then build a budgeted
    /// context pack: target first, neighbors ordered ascending by pagerank
    /// (highest-centrality lands last — "lost in the middle").
    ///
    /// `max_tokens` is a hard ceiling on the estimated size of the returned
    /// pack text (`manifest::token_estimate`, chars/4): every block is
    /// charged at the size of the text actually emitted. A code target is
    /// its outline, cut to the budget. Another target is its full page;
    /// when that alone would blow the ceiling, it degrades to a summary
    /// block (title + summary + a pointer at `wiki://page/<id>`) so the
    /// neighborhood still fits. The one floor exception: the outline's
    /// header or the degraded block is always emitted, even when a
    /// pathologically small budget cannot contain it — a neighbors call
    /// always says something about the target.
    ///
    /// Neighbor selection walks the target's import neighbors (either
    /// direction) first, then the rest, each group in descending centrality,
    /// and keeps whichever fit the remaining budget (skip, not break), so
    /// the kept set is the highest-centrality set the greedy can admit —
    /// deliberately NOT a maximum-cardinality fill. A heavier high-
    /// centrality neighbor beats several lighter lower-centrality ones; see
    /// `full_neighbors_max_tokens_prefers_centrality_over_packing`.
    /// An import neighbor is its whole outline when that fits, else a
    /// summary block like every other neighbor. With `full_neighbors`,
    /// every neighbor is its whole outline (code) or full page (other).
    pub fn neighbors(&self, id: &str, depth: usize, budget: &PackBudget) -> Option<ContextPack> {
        let target = self.entries.get(id)?;

        let seen = self.bfs_neighborhood(id, depth);
        let imports: BTreeSet<&str> = target
            .imports_out
            .iter()
            .chain(&target.imports_in)
            .map(String::as_str)
            .collect();

        // Candidates (excluding target, of `kind` when given): import
        // neighbors first, then descending by pagerank, so every budget
        // below always considers them, and the highest-centrality neighbor,
        // first.
        let mut candidates: Vec<String> = seen
            .iter()
            .filter(|n| *n != id)
            .filter(|n| {
                budget.kind.as_ref().is_none_or(|k| {
                    self.entries
                        .get(*n)
                        .and_then(|e| SourceKind::parse(&e.kind))
                        .is_some_and(|sk| k.matches(&sk))
                })
            })
            .cloned()
            .collect();
        candidates.sort_by(|a, b| {
            imports
                .contains(b.as_str())
                .cmp(&imports.contains(a.as_str()))
                .then_with(|| self.pagerank_of(b).total_cmp(&self.pagerank_of(a)))
                .then_with(|| a.cmp(b))
        });

        // Apply max_nodes: keep target + the highest-centrality neighbors
        // (the head of the descending list); the lowest-centrality tail is
        // dropped first.
        if let Some(max) = budget.max_nodes {
            let keep = max.saturating_sub(1);
            candidates.truncate(keep);
        }

        // Budget math tracks emitted *chars* and converts with the same
        // chars/4 rule as `manifest::token_estimate` — summing per-block
        // token estimates would under-count the concatenation by up to one
        // token per block (floor division) and break the ceiling. The
        // integration test `max_tokens_is_a_hard_ceiling_on_pack_size`
        // pins the two implementations together.
        let fits = |chars: usize| match budget.max_tokens {
            Some(b) => chars / 4 <= b,
            None => true,
        };

        // Target block: a code page's outline, cut so that it and the
        // separator fit (the separator's 7 chars cost at most 2 tokens on
        // top of the outline's own count). Another page in full, degraded
        // to a summary block when it (plus separator) alone would blow the
        // ceiling.
        let full_target = self
            .page(id)
            .unwrap_or_else(|| format!("# {}\n", target.title));
        let sep_chars = Self::SEPARATOR.chars().count();
        let target_block = if self.is_code(id) {
            self.outline(id, budget.max_tokens.map(|m| m.saturating_sub(2)))?
        } else if fits(full_target.chars().count() + sep_chars) {
            full_target
        } else {
            format!(
                "# {} ({id})\n\n{}\n\n_body (~{} tokens) {OVER_BUDGET_NOTE}; read wiki://page/{id} for the full page_\n",
                target.title,
                target.summary.as_deref().unwrap_or("(no summary)"),
                target.token_estimate,
            )
        };
        // Floor: the target block is charged but never dropped.
        let mut chars_used = target_block.chars().count() + sep_chars;

        // Admit neighbors in candidate order, each charged at the size of
        // the block actually emitted (outline, summary line or full page).
        let mut kept: Vec<(String, String)> = Vec::new();
        for nid in candidates {
            let Some(e) = self.entries.get(&nid) else {
                continue;
            };
            let summary = || {
                format!(
                    "## {} ({})\n{}\n\n",
                    e.title,
                    nid,
                    e.summary.as_deref().unwrap_or("(no summary)")
                )
            };
            let outline = || self.outline(&nid, None).map(|o| o + "\n");
            let block = if budget.full_neighbors {
                if self.is_code(&nid) {
                    outline()
                } else {
                    self.page(&nid)
                }
                .unwrap_or_default()
            } else if imports.contains(nid.as_str()) {
                outline()
                    .filter(|o| fits(chars_used + o.chars().count()))
                    .unwrap_or_else(summary)
            } else {
                summary()
            };
            let cost = block.chars().count();
            if !fits(chars_used + cost) {
                continue;
            }
            chars_used += cost;
            kept.push((nid, block));
        }

        // Emit ascending by pagerank so the highest-centrality neighbor
        // lands last ("lost in the middle" placement).
        kept.sort_by(|(a, _), (b, _)| {
            self.pagerank_of(a)
                .total_cmp(&self.pagerank_of(b))
                .then_with(|| a.cmp(b))
        });

        let mut text = String::new();
        let mut included = vec![id.to_string()];
        text.push_str(&target_block);
        text.push_str(Self::SEPARATOR);
        for (nid, block) in kept {
            text.push_str(&block);
            included.push(nid);
        }

        Some(ContextPack { text, included })
    }
}

// A whole-name match must outrank any word match, on the page and on the
// line, even when the word's side also matches by doc; a title match must
// outrank a whole-name match.
const _: () = assert!(
    Wiki::W_SYMBOL < Wiki::W_NAME
        && Wiki::W_SYMBOL > Wiki::W_DEFINED + Wiki::W_SYMBOL_DOC
        && Wiki::W_SYMBOL > Wiki::W_SYMBOL_WORD + Wiki::W_SYMBOL_DOC
        && Wiki::W_SYMBOL > Wiki::W_METHOD_WORD + Wiki::W_SYMBOL_DOC
        && Wiki::W_SYMBOL_WORD > Wiki::W_SYMBOL_DOC + Wiki::W_SYMBOL_DOC_REST
        && Wiki::IW_NAME_WORD > Wiki::IW_DOC + Wiki::IW_DOC_REST
        && Wiki::IW_WHOLE > Wiki::IW_NAME_WORD + Wiki::IW_DOC
        && Wiki::IW_WHOLE > Wiki::IW_METHOD_WORD + Wiki::IW_DOC
);

#[cfg(test)]
mod tests {
    use super::{query_dates, QueryDate, Wiki};
    use crate::model::{CodeItem, CodeItemKind};
    use std::collections::BTreeSet;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(std::string::ToString::to_string).collect()
    }

    #[test]
    fn stem_cuts_a_cyrillic_ending_but_keeps_four_letters() {
        for (word, stem) in [
            ("конкурентів", "конкурент"),
            ("конкуренти", "конкурент"),
            ("куратором", "куратор"),
            ("куратор", "куратор"),
            ("агенти", "агент"),
            ("позначки", "позначк"),
            ("мова", "мова"),
            ("routes", "routes"),
            ("mcp-сервери", "mcp-сервер"),
        ] {
            assert_eq!(Wiki::stem(word), stem, "{word}");
        }
    }

    #[test]
    fn query_tokens_stem_dedupe_and_keep_the_raw_words() {
        let owned = |v: &[&str]| v.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            Wiki::query_tokens("агента агентом"),
            (owned(&["агент"]), owned(&["агента", "агентом"]))
        );
        assert_eq!(
            Wiki::query_tokens("MCP resources"),
            (owned(&["mcp", "resources"]), owned(&["mcp", "resources"]))
        );
    }

    #[test]
    fn tokenize_lowercases_splits_and_trims_punctuation_edges() {
        assert_eq!(Wiki::tokenize("MCP resources"), vec!["mcp", "resources"]);
        assert_eq!(Wiki::tokenize("Serve,"), vec!["serve"]);
        // Interior non-alphanumerics survive.
        assert_eq!(Wiki::tokenize("source_hash"), vec!["source_hash"]);
        assert_eq!(Wiki::tokenize("code:rust"), vec!["code:rust"]);
    }

    #[test]
    fn tokenize_dedupes_and_drops_empty() {
        assert_eq!(Wiki::tokenize("beta BETA beta"), vec!["beta"]);
        assert!(Wiki::tokenize("").is_empty());
        assert!(Wiki::tokenize("  ,, !! ").is_empty());
    }

    #[test]
    fn field_hit_needs_every_token_part_as_a_word_prefix() {
        let w = Wiki::field_words;
        // Words are maximal alphanumeric runs; digits count, `_ - :` split.
        assert_eq!(
            w("wiki::search 2026-09 a_b"),
            vec!["wiki", "search", "2026", "09", "a", "b"]
        );
        // No longer a substring hit inside a word.
        assert!(!Wiki::field_hit(&w("rewrite"), "write"));
        assert!(!Wiki::field_hit(&w("process-wide query cache"), "id"));
        // A prefix of a word still hits (derived forms survive).
        assert!(Wiki::field_hit(&w("atomic writes"), "write"));
        assert!(Wiki::field_hit(&w("directory walk"), "dir"));
        // Every part of a punctuated token must hit; parts may match
        // non-adjacent words.
        assert!(Wiki::field_hit(
            &w("process wide query cache"),
            "process-wide"
        ));
        assert!(Wiki::field_hit(&w("search the whole wiki"), "wiki::search"));
        assert!(!Wiki::field_hit(&w("process query"), "process-wide"));
        // Unicode alphanumerics are words too.
        assert!(Wiki::field_hit(&w("naïve bayes"), "na"));
        // Longer than the word: not a prefix.
        assert!(!Wiki::field_hit(&w("ab"), "abc"));
    }

    #[test]
    fn name_words_splits_snake_camel_and_acronym_runs() {
        assert_eq!(Wiki::name_words("PackBudget"), vec!["pack", "budget"]);
        assert_eq!(Wiki::name_words("CACHE_VERSION"), vec!["cache", "version"]);
        assert_eq!(Wiki::name_words("HTTPServer"), vec!["http", "server"]);
        assert_eq!(Wiki::name_words("has_page"), vec!["has", "page"]);
        assert_eq!(Wiki::name_words("walk"), vec!["walk"]);
    }

    #[test]
    fn defined_terms_keep_full_name_and_words_minus_title_words() {
        // Word-level self-name skip: the title word `code` goes, the full
        // name stays (the title never matches the full-name token), the
        // other word stays.
        assert_eq!(
            Wiki::defined_terms("code", &["CodeExtractor".into(), "load".into()], &[]),
            set(&["codeextractor", "extractor", "load"])
        );
        // A name that IS the title word contributes nothing.
        assert_eq!(
            Wiki::defined_terms("cache", &["Cache".into()], &[]),
            set(&[])
        );
        // Multi-word title: every title word is skipped.
        assert_eq!(
            Wiki::defined_terms(
                "graph page",
                &["build_graph".into(), "page_ids".into()],
                &[]
            ),
            set(&["build", "build_graph", "ids", "page_ids"])
        );
        assert_eq!(
            Wiki::defined_terms("hash", &["hash_bytes".into()], &[]),
            set(&["bytes", "hash_bytes"])
        );
    }

    #[test]
    fn method_names_enter_the_term_set_whole_and_never_word_split() {
        // `needs_render` must not hand the page the word `render` (the
        // 2026-09-03 spike measured `render fingerprint` leaving `rewrite`
        // for `cache` when it did); a name equal to a title word is dropped
        // like a defined full name.
        assert_eq!(
            Wiki::defined_terms(
                "src query",
                &[],
                &["has_page".into(), "needs_render".into(), "query".into()]
            ),
            set(&["has_page", "needs_render"])
        );
    }

    fn item(kind: CodeItemKind, name: &str, doc: Option<&str>, test: bool) -> CodeItem {
        CodeItem {
            kind,
            name: name.into(),
            owner: None,
            signature: String::new(),
            start_line: 1,
            end_line: 1,
            doc: doc.map(Into::into),
            doc_text: None,
            public: false,
            twin: None,
            test,
            calls: Vec::new(),
            call_sites: Vec::new(),
        }
    }

    #[test]
    fn tail_scores_only_words_the_first_sentence_misses() {
        use CodeItemKind::Fn;
        let mut a = item(Fn, "alpha", Some("Opens the gate."), false);
        let b = item(Fn, "beta", None, false);
        let score = |i: &CodeItem, q: &[&str]| {
            let t: Vec<String> = q.iter().map(|s| (*s).into()).collect();
            Wiki::item_score(i, &t, &vec![1.0; t.len()])
        };
        let head = score(&a, &["opens", "gate"]);
        a.doc_text = Some("Opens the gate. Refuses when a position is open.".into());
        assert!(score(&a, &["refuses", "position"]) > score(&b, &["refuses", "position"]));
        assert!((score(&a, &["opens", "gate"]) - head).abs() < 1e-12);
    }

    #[test]
    fn old_items_json_without_doc_text_loads() {
        let json = r#"{"kind":"fn","name":"f","signature":"fn f()","start_line":1,"end_line":1,"doc":"X.","public":true}"#;
        let i: CodeItem = serde_json::from_str(json).unwrap();
        assert_eq!(i.doc_text, None);
    }

    #[test]
    fn symbol_fields_skip_own_title_defined_terms_tests_and_modules() {
        use CodeItemKind::{Fn, Method, Module};
        let items = [
            // Another page's title word (`summary`) stays.
            item(Fn, "summary_suffix", None, false),
            item(Fn, "render", None, false),
            // Own title word and a defined term go.
            item(Fn, "cache_store", Some("Writes Atomically."), false),
            // Method: whole name, words in their own field minus name terms.
            item(Method, "needs_render", None, false),
            item(Method, "suffix_of", None, false),
            item(Fn, "zebra", Some("Zebra doc."), true),
            item(Module, "formats", None, false),
        ];
        let f = Wiki::symbol_fields("cache", &set(&["store"]), &items);
        assert_eq!(
            f.whole,
            set(&[
                "cache_store",
                "needs_render",
                "render",
                "suffix_of",
                "summary_suffix"
            ])
        );
        assert_eq!(f.words, set(&["suffix", "summary"]));
        assert_eq!(f.method_words, set(&["needs", "of"]));
        assert_eq!(f.doc_words, ["writes", "atomically"]);
    }

    fn d(year: Option<u32>, month: u32, day: u32) -> QueryDate {
        QueryDate { year, month, day }
    }

    #[test]
    fn query_dates_reads_iso_dotted_and_month_name_forms() {
        let ok = [
            ("2026-10-12", d(Some(2026), 10, 12)),
            ("12.10", d(None, 10, 12)),
            ("03.10", d(None, 10, 3)),
            ("12.10.", d(None, 10, 12)),
            ("12.10.2026", d(Some(2026), 10, 12)),
            ("12 жовтня", d(None, 10, 12)),
            ("12 Жовтня 2025", d(Some(2025), 10, 12)),
            ("жовтень 12", d(None, 10, 12)),
            ("що сталося 12 жовтня?", d(None, 10, 12)),
            ("October 12", d(None, 10, 12)),
            ("October 12, 2026", d(Some(2026), 10, 12)),
            ("12 oct", d(None, 10, 12)),
            ("sept 3", d(None, 9, 3)),
            ("3 марта", d(None, 3, 3)),
            ("1 января", d(None, 1, 1)),
            ("29 лютого", d(None, 2, 29)),
            ("29 feb 2024", d(Some(2024), 2, 29)),
        ];
        for (q, want) in ok {
            assert_eq!(query_dates(q), [want], "{q}");
        }
        for q in [
            "v1.12",
            "1.2.3",
            "0.4",
            "2026",
            "12",
            "жовтня",
            "port 8080",
            "31 лютого",
            "29 feb 2023",
            "october",
            "32 oct",
            "2026-02-30",
            "1.2",
            "12.13",
            "3.12",
            "1.10",
            "python 3.12",
            "3.11",
        ] {
            assert!(query_dates(q).is_empty(), "{q}");
        }
        assert_eq!(
            query_dates("12.10 and 12 жовтня and 5 oct"),
            [d(None, 10, 12), d(None, 10, 5)]
        );
    }
}
