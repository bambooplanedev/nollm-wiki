//! Query surface over a compiled wiki: search, page lookup, neighbor packs.
//!
//! `leading_doc` takes only the first line of this comment as the page
//! summary, so that line has to stand on its own. Detail goes below it:
//! `neighbors` walks outward from a page and stops at a caller-supplied
//! token ceiling, degrading an oversized target to a title-and-summary block.

use crate::calls::{callers_index, Caller};
use crate::model::{is_test_path, CodeItem, KindFilter, Origin, SourceKind};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

mod dates;
mod resolve;
mod score;
mod search;
mod tokens;

pub(crate) use dates::{query_dates, QueryDate};

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
    /// `repo` or `local` on a memory note; absent on other pages and in an
    /// `index.json` older than the field (see `Entry::origin`).
    #[serde(default)]
    pub(crate) origin: Option<String>,
    #[serde(default)]
    pub(crate) author: Option<String>,
    #[serde(default)]
    pub(crate) edited: bool,
    #[serde(default)]
    pub(crate) mine: bool,
    /// The id of the note that supersedes this one; absent in `index.json` when unset.
    #[serde(default)]
    pub(crate) superseded_by: Option<String>,
    pub(crate) pagerank: f64,
    pub(crate) token_estimate: usize,
    pub(crate) neighbors_out: Vec<String>,
    pub(crate) neighbors_in: Vec<String>,
    /// The import edges among `neighbors_out` and `neighbors_in`. Defaulted:
    /// an older `index.json` has no such keys, and `serve` reads wikis it did
    /// not compile.
    #[serde(default)]
    pub(crate) imports_out: Vec<String>,
    #[serde(default)]
    pub(crate) imports_in: Vec<String>,
}

impl Entry {
    /// The note's origin; an `index.json` older than the field gets it
    /// guessed from the path: a relative (repo) path is `repo` without an
    /// author, an absolute (agent folder) one `local`.
    pub(crate) fn origin(&self) -> Origin {
        let repo = match self.origin.as_deref() {
            _ if !self.memory => false,
            Some(o) => o == "repo",
            None => !self.path.starts_with('/') && !Path::new(&self.path).is_absolute(),
        };
        if repo {
            Origin::Repo {
                author: self.author.clone(),
                edited: self.edited,
                mine: self.mine,
            }
        } else {
            Origin::Local
        }
    }

    /// Whether the page's source kind passes `f`; `tests` also needs a test
    /// path (`is_test_path`).
    fn is_kind(&self, f: &KindFilter) -> bool {
        SourceKind::parse(&self.kind).is_some_and(|k| f.matches(&k))
            && (*f != KindFilter::Tests || is_test_path(&self.path))
    }

    /// Whether the page passes a search's `kind` (a memory note passes the
    /// `code` and `docs` groups whatever its real kind) and `tag` (it or a
    /// sub-tag of it).
    fn passes(&self, kind: Option<&KindFilter>, tag: Option<&str>) -> bool {
        kind.is_none_or(|f| {
            (self.memory && matches!(f, KindFilter::Code | KindFilter::Docs)) || self.is_kind(f)
        }) && tag.is_none_or(|t| {
            self.tags
                .iter()
                .any(|g| g == t || g.strip_prefix(t).is_some_and(|r| r.starts_with('/')))
        })
    }

    /// `passes`, and not a test page when `kind` drops them
    /// (`KindFilter::drops_tests`): what a `search` lists. A memory note is
    /// never a test page.
    fn shown(&self, kind: Option<&KindFilter>, tag: Option<&str>) -> bool {
        self.passes(kind, tag)
            && (self.memory
                || !kind.is_some_and(KindFilter::drops_tests)
                || !is_test_path(&self.path))
    }
}

#[derive(Deserialize)]
struct IndexFile {
    /// `Manifest::root`; empty for an `index.json` older than the field.
    #[serde(default)]
    root: String,
    entries: Vec<Entry>,
}

/// `arg` as a path: `\` read as `/`, one leading `./` dropped.
fn norm_path(arg: &str) -> String {
    let norm = arg.replace('\\', "/");
    norm.strip_prefix("./").unwrap_or(&norm).to_string()
}

/// The error for an id `resolve_id` could not place; `near` are its
/// candidates and `defined` the places that define an item named like the
/// argument's file stem.
pub(crate) fn unknown_page(arg: &str, near: &[String], defined: &[String]) -> String {
    let mut parts = String::new();
    if !near.is_empty() {
        parts += &format!("; closest: {}", near.join(", "));
    }
    if !defined.is_empty() {
        parts += &format!("; defined in: {}", defined.join(", "));
    }
    format!("unknown page {arg:?}{parts} — use search to find ids")
}

/// A code page's `## Body` without the fence lines `render_page` wraps it in,
/// so the language word in the info string is not searchable text and never
/// opens a snippet. Anything not fenced that way (pages from <= 0.6.7) is
/// returned as is.
fn unfence(body: &str) -> &str {
    let fence_len = body.bytes().take_while(|&b| b == b'`').count();
    let Some((open, rest)) = body.split_once('\n') else {
        return body;
    };
    let closing = &"`".repeat(fence_len.max(3));
    match rest.strip_suffix(closing.as_str()) {
        Some(inner) if fence_len >= 3 && !open[fence_len..].contains('`') => {
            inner.strip_suffix('\n').unwrap_or(inner)
        }
        _ => body,
    }
}

impl Wiki {
    /// Why `page_text`/`page` found no body for `id`: an indexed page whose
    /// file is gone says so; anything else is an unknown page named `arg`.
    pub fn page_error(&self, id: &str, arg: &str) -> String {
        if self.has_page(id) && self.page(id).is_none() {
            format!("page {id:?} is in the index but its file {id}.md is missing from the wiki folder — recompile the wiki")
        } else {
            unknown_page(arg, &[], &[])
        }
    }
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

/// Most memory notes `search` lists after the code hits.
pub(crate) const MEMORY_TAIL: usize = 2;
/// Default gate length: a query word this long naming a note lets it in.
/// 4 from the agent runs (2026-11-01, Claude Haiku on 30 memory and 8 code
/// tasks): Haiku searches with short code-shaped words, so at 8 the note
/// never appeared in 14 of 60 runs and 42 were right against 51 with the
/// old boost; at 4, 49 of 60 (parity) with the code list still untouched.
/// Offline, 4 puts a note under most code queries (about +540 bytes per
/// answer on this repo); 8 under 3-32% but found 75 of 114 notes to 103.
pub(crate) const MEMORY_GATE_LEN: usize = 4;
/// A note whose score is at least this many times the best code hit's
/// passes the gate without a name.
pub(crate) const MEMORY_GATE_RATIO: f64 = 2.0;

/// The gate length: `WIKI_MEMORY_GATE` when set to a number, else
/// `MEMORY_GATE_LEN`; read once per process.
pub(crate) fn gate_len() -> usize {
    static LEN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *LEN.get_or_init(|| {
        std::env::var("WIKI_MEMORY_GATE")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|n| *n > 0)
            .unwrap_or(MEMORY_GATE_LEN)
    })
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
    /// The page is an agent memory note (a tail row of `search`).
    pub memory: bool,
    /// Where a memory note came from; `Local` on every other page.
    pub origin: Origin,
    /// The id of the note that supersedes this one; it never enters the tail.
    pub superseded_by: Option<String>,
    /// Deterministic excerpt around the earliest body match (`None` for
    /// hits that matched only title, alias, item name, summary, or a
    /// section heading — the summary explains those).
    pub snippet: Option<String>,
    /// The page's best-matching code items, best first; empty for docs pages
    /// and for code pages that matched without any item.
    pub symbols: Vec<SymbolLine>,
}

impl Hit {
    /// A hit for `e` with no symbol lines yet.
    fn from_entry(e: &Entry, score: f64, snippet: Option<String>) -> Hit {
        Hit {
            id: e.id.clone(),
            title: e.title.clone(),
            path: e.path.clone(),
            summary: e.summary.clone(),
            updated: e.updated.clone(),
            score,
            memory: e.memory,
            origin: e.origin(),
            superseded_by: e.superseded_by.clone(),
            snippet,
            symbols: Vec::new(),
        }
    }
}

/// One code item shown under its page's hit.
#[derive(Clone)]
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
    /// A function nested in another (`CodeItem::nested`).
    pub nested: bool,
    /// The one other definition of a function's name elsewhere in the
    /// project, as `(path, start_line, end_line, name)`; see `same_name`.
    pub also: Option<(String, usize, usize, String)>,
    /// How many functions call it, as `callers` lists them one level up;
    /// set on a function's line in a query of names alone, when any does.
    pub called_by: Option<usize>,
}

/// A searchable field: its words (`Wiki::field_words`) and their stems,
/// computed once per field so the stem tier compares strings, not stemmer
/// output, per query token.
#[derive(Default, Debug)]
struct Field {
    words: Vec<String>,
    stems: Vec<String>,
}

impl Field {
    /// The field of a lowercased text.
    fn new(lower: &str) -> Field {
        Field::of(Wiki::field_words(lower))
    }

    fn of(words: Vec<String>) -> Field {
        let stems = words.iter().map(|w| Wiki::stem(w)).collect();
        Field { words, stems }
    }
}

/// The search fields a page earns from its items (`Wiki::symbol_fields`),
/// built once in `Wiki::load`.
#[derive(Default)]
struct SymbolFields {
    /// Whole item names, lowercased, each also as `owner::name`.
    whole: BTreeSet<String>,
    /// Words of the names, none of them in `whole` or the page's title.
    words: BTreeSet<String>,
    /// The stems of `words`, for the stem tier.
    word_stems: BTreeSet<String>,
    /// Words of the items' docs.
    doc: Field,
    /// Words of the items' whole docs, scored below `doc`.
    doc_rest: Field,
    /// The word sets of multi-word item names (`name_words`, unfiltered).
    name_sets: BTreeSet<BTreeSet<String>>,
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

/// Token ceiling applied to the `neighbors` tool when the caller omits
/// `max_tokens`. The CLI stays unbounded: a person passes flags on purpose,
/// an agent calling with defaults should not receive a 30 KB dump.
pub const DEFAULT_NEIGHBORS_MAX_TOKENS: usize = 4000;

/// What `Wiki::search` learns about one page in its first pass. Scores
/// need corpus-wide statistics (`df`, `avglen`), so per-page facts are
/// held until every page has been seen; the page text itself is not.
struct Candidate<'a> {
    entry: &'a Entry,
    /// Per query token, in query order: (summed field weights, body
    /// occurrences).
    per_token: Vec<(f64, usize)>,
    /// The tokens that name this page's file.
    file_tokens: Vec<usize>,
    /// Whitespace-word count of the searchable content.
    len: usize,
    snippet: Option<String>,
}

/// Pass-1 statistics of `search_terms` over the pages it counts: how many,
/// per token (then date) how many it hits, and their total word count.
#[derive(Default, Clone)]
pub(crate) struct PassStats {
    n: usize,
    df: Vec<usize>,
    total_len: usize,
    /// Per token, the pages whose file it names (`Wiki::names_file`).
    named: Vec<usize>,
    /// For a prompt: the summed idf of the words that count, what a page
    /// hit once by each of them at weight 1 would score.
    available: f64,
    /// For a prompt: the words that count.
    terms: Vec<String>,
    /// A `code` search left a test page out of its top `limit`.
    tests_cut: bool,
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
    /// Each function's callers by its `path:start-end` (`callers_index`).
    callers: BTreeMap<String, Vec<Caller>>,
    /// Non-test function/method definitions by unqualified name, as
    /// (page id, item index); built on first use by `same_name`.
    by_name: std::sync::OnceLock<HashMap<String, Vec<(String, usize)>>>,
}

impl Wiki {
    /// Load a compiled output directory: reads `index.json` for metadata +
    /// adjacency and reads every page body from `<id>.md` once; a page whose
    /// file is missing has no body. `items.json` is read the same way: missing
    /// or invalid means no items.
    pub fn load(dir: &Path) -> Result<Wiki, std::io::Error> {
        let file = dir.join("index.json");
        let hint = format!("run `wiki compile <input> {}`", dir.display());
        let text = std::fs::read_to_string(&file).map_err(|e| {
            let msg = if e.kind() == std::io::ErrorKind::NotFound {
                format!("{} not found — {hint}", file.display())
            } else {
                format!("{}: {e}", file.display())
            };
            std::io::Error::new(e.kind(), msg)
        })?;
        let index: IndexFile = serde_json::from_str(&text).map_err(|e| {
            let msg = format!("{}: {e} — {hint}", file.display());
            std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
        })?;
        let entries: BTreeMap<String, Entry> = index
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
        let symbol_fields = entries
            .values()
            .filter_map(|e| {
                let items = items.get(&e.id)?;
                let title = e.title.to_lowercase();
                Some((e.id.clone(), Self::symbol_fields(&title, items)))
            })
            .collect();
        let callers = callers_index(
            items
                .iter()
                .filter_map(|(id, its)| Some((entries.get(id)?.path.as_str(), its.as_slice()))),
        );
        Ok(Wiki {
            root: index.root,
            entries,
            bodies,
            items,
            symbol_fields,
            callers,
            by_name: std::sync::OnceLock::new(),
        })
    }

    /// A page's code items, as compiled; empty when it has none.
    pub fn items(&self, id: &str) -> &[CodeItem] {
        self.items.get(id).map_or(&[], Vec::as_slice)
    }

    /// The functions that call the one at `span` (`path:start-end`).
    pub(crate) fn callers_of(&self, span: &str) -> &[Caller] {
        self.callers.get(span).map_or(&[], Vec::as_slice)
    }

    /// Every page with code items: its source path and its items.
    pub(crate) fn code_items(&self) -> impl Iterator<Item = (&str, &[CodeItem])> {
        self.items
            .iter()
            .filter_map(|(id, items)| Some((self.entries.get(id)?.path.as_str(), items.as_slice())))
    }

    /// Whether `id` is an agent memory note.
    pub(crate) fn is_memory(&self, id: &str) -> bool {
        self.entries.get(id).is_some_and(|e| e.memory)
    }

    /// A page's rendered Markdown body by id, as it was when loaded.
    pub fn page(&self, id: &str) -> Option<String> {
        self.bodies.get(id).cloned()
    }

    /// Every page's rendered Markdown body by id, as it was when loaded.
    pub(crate) fn bodies(&self) -> &BTreeMap<String, String> {
        &self.bodies
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
    /// `## NOTES` section.
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
    // Added for a token that is the page's file name or a path suffix of it
    // (`names_file`), times `idf` of the N pages it names and over `sqrt(N)`: a
    // name on many pages (`mod.rs`) must not crowd out the answer to the query's
    // other words, and `df` would sink a header many files include. Measured
    // 2026-11-01 on 3 local projects, 866 queries naming a unique file: first
    // in 778 (19 before), top 5 in all (126 before), the other answer's top 10
    // unchanged. 120 naming a shared name: that answer's top 5 106 of 108 (base
    // 108); a sort key cut it to 26, plain `df` idf at 12.0 to 81.
    const W_FILE: f64 = Self::W_NAME + Self::W_TITLE;
    const W_ALIAS: f64 = 2.0;
    // Page fields from code items (`symbol_fields`), private and method
    // items included, test and module items never. A whole item name
    // scores `W_SYMBOL`, a word of one `W_SYMBOL_WORD`; `W_SYMBOL` sits
    // above the word weight plus the doc weight and below `W_NAME` (checked at compile time
    // in `score.rs`), so a page naming the query beats a page whose names merely
    // contain it, and a page titled by the query beats both. Tuned on the
    // symbol eval's `tune` split (2026-10-23): `W_SYMBOL_DOC` 0.5 → 0.75
    // moved a doc query to file@1; 1.0 cost a title query. `W_SYMBOL` 3.0 →
    // 2.8 restored the `W_NAME` rule and moved no eval rank.
    const W_SYMBOL: f64 = 2.8;
    const W_SYMBOL_WORD: f64 = 2.0;
    // Per word, when all the query's words sit inside one longer item name.
    const W_IN_ONE_NAME: f64 = 1.0;
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

    /// The share of a field's weight a stem-only hit earns: below an exact
    /// or prefix hit, so `sorted` still prefers the page that says `sorted`
    /// over one that says `sorting`. 0.3 measured 2026-10-31 against 0.5:
    /// same symbol and retrieval evals, hint precision 12/18 vs 11/18.
    pub(crate) const STEM_TIER: f64 = 0.3;
}

#[cfg(test)]
mod tests;
