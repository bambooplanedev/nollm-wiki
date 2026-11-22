//! Scoring a page against a query: fields, item scores, snippets, candidates.

use super::{query_dates, unfence, Candidate, Entry, Field, QueryDate, SymbolFields, Wiki};
use crate::model::{CodeItem, CodeItemKind};
use aho_corasick::AhoCorasick;
use std::collections::BTreeSet;

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

impl Wiki {
    /// Whether any query token of `q` (as typed, as `search` reads it) hits
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
            .map(|f| Field::new(&f.to_lowercase()))
            .collect::<Vec<_>>();
        Self::tokenize(q)
            .iter()
            .filter(|t| t.chars().count() >= min_len)
            .any(|t| fields.iter().any(|w| Self::field_tier(w, t) > 0.0))
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

    /// The fields a page earns from its non-test, non-module items. Whole
    /// names and words lose the page's own title words; an owned item's
    /// whole name also enters as `owner::name`; a word that is also a whole
    /// name scores as the whole name only. Another page's title words stay: dropping them hid `onOrderReject` from
    /// `order reject` on a 2,700-page wiki; `W_TITLE` guards title queries.
    pub(super) fn symbol_fields(title_lower: &str, items: &[CodeItem]) -> SymbolFields {
        let title_words: Vec<&str> = title_lower.split_whitespace().collect();
        let mut f = SymbolFields::default();
        for item in items
            .iter()
            .filter(|i| !i.test && !matches!(i.kind, CodeItemKind::Module | CodeItemKind::TopLevel))
            // A shell global (a nested value) is found by its exact name only.
            .filter(|i| !(i.nested && i.kind == CodeItemKind::Value))
        {
            let whole = item.name.to_lowercase();
            if !title_words.contains(&whole.as_str()) {
                f.whole.insert(whole);
            }
            if item.owner.is_some() {
                f.whole.insert(item.qualified_name().to_lowercase());
            }
            let words = Self::name_words(&item.name);
            if words.len() > 1 {
                f.name_sets.insert(words.iter().cloned().collect());
            }
            f.words.extend(
                Self::match_words(&item.name)
                    .into_iter()
                    .filter(|w| !title_words.contains(&w.as_str())),
            );
            f.doc.words.extend(Self::lower_words(item.doc.as_deref()));
            f.doc_rest
                .words
                .extend(Self::lower_words(item.doc_text.as_deref()));
        }
        f.words.retain(|w| !f.whole.contains(w));
        f.word_stems = f.words.iter().map(|w| Self::stem(w)).collect();
        f.doc = Field::of(std::mem::take(&mut f.doc.words));
        f.doc_rest = Field::of(std::mem::take(&mut f.doc_rest.words));
        f
    }

    // Item scores for the symbol lines under a hit: per query token,
    // `idf · (whole name or name word) + idf · doc`, test items scaled down;
    // a query made of exactly the name's words scores every token as whole.
    // A whole-name match outranks any word match, even one whose item also
    // matches by doc (checked at compile time below): at 2.5, graph.rs
    // `resolve_link`'s word plus its doc put it above the method `resolve`.
    const IW_WHOLE: f64 = 3.5;
    const IW_NAME_WORD: f64 = 2.0;
    const IW_DOC: f64 = 1.0;
    const IW_DOC_REST: f64 = Self::IW_DOC / 2.0;
    const TEST_ITEM_FACTOR: f64 = 0.5;
    /// Symbol lines per page at most.
    const SYMBOLS_PER_PAGE: usize = 3;

    pub(super) fn item_score(
        item: &CodeItem,
        tokens: &[String],
        query_words: &BTreeSet<String>,
        idf: &[f64],
    ) -> f64 {
        let whole = item.name.to_lowercase();
        let qualified = item.qualified_name().to_lowercase();
        let words = Self::name_words(&item.name);
        // The query's words are this item's name words, in any order.
        let spelled =
            words.len() > 1 && words.iter().cloned().collect::<BTreeSet<_>>() == *query_words;
        let words = Self::match_words(&item.name);
        let doc = Field::of(Self::lower_words(item.doc.as_deref()));
        let doc_rest = Field::of(Self::lower_words(item.doc_text.as_deref()));
        let score: f64 = tokens
            .iter()
            .zip(idf)
            .map(|(t, &idf_t)| {
                let name = if spelled || *t == whole || *t == qualified {
                    Self::IW_WHOLE
                } else if words.contains(t) {
                    Self::IW_NAME_WORD
                } else if Self::shares_stem(&words, t) {
                    Self::IW_NAME_WORD * Self::STEM_TIER
                } else {
                    0.0
                };
                let doc_tier = Self::field_tier(&doc, t);
                let doc = if doc_tier > 0.0 {
                    Self::IW_DOC * doc_tier
                } else {
                    Self::IW_DOC_REST * Self::field_tier(&doc_rest, t)
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
    pub(super) fn page_symbols<'a>(
        &'a self,
        id: &str,
        tokens: &[String],
        query_words: &BTreeSet<String>,
        idf: &[f64],
    ) -> Vec<(f64, &'a CodeItem)> {
        let mut scored: Vec<(f64, &CodeItem)> = self
            .items(id)
            .iter()
            .filter(|i| !matches!(i.kind, CodeItemKind::Module | CodeItemKind::TopLevel))
            .filter(|i| !(i.nested && i.kind == CodeItemKind::Value))
            .map(|i| (Self::item_score(i, tokens, query_words, idf), i))
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

    /// Per pattern of `bodies` (a token): its non-overlapping occurrences in
    /// `text`, what `str::match_indices` counts, and the byte offset of the
    /// first, what `str::find` gives; one pass over `text` for all of them.
    /// Overlapping matches arrive in order of their end, so for one pattern
    /// in order of their start, and the greedy skip reproduces
    /// `match_indices`.
    pub(super) fn body_counts(
        bodies: &AhoCorasick,
        text: &str,
        n: usize,
    ) -> (Vec<usize>, Vec<Option<usize>>) {
        let mut tf = vec![0; n];
        let mut first = vec![None; n];
        let mut next = vec![0; n];
        for m in bodies.find_overlapping_iter(text) {
            let i = m.pattern().as_usize();
            if m.start() >= next[i] {
                tf[i] += 1;
                next[i] = m.end();
                if first[i].is_none() {
                    first[i] = Some(m.start());
                }
            }
        }
        (tf, first)
    }

    /// Excerpt around the earliest occurrence of any query token in the
    /// content (`first`: per token, its offset in the lowercased content, or
    /// none): 60 chars of context each side, whitespace runs collapsed,
    /// `…` on truncated edges. Match offsets come from the lowercased text;
    /// on the rare non-ASCII page where lowercasing changes byte lengths the
    /// window may sit slightly off, but boundary snapping guarantees it
    /// never panics and stays deterministic.
    fn snippet(content: &str, tokens: &[String], first: &[Option<usize>]) -> Option<String> {
        // Earliest occurrence of any token; ties at the same index go to the
        // longer token.
        let mut best: Option<(usize, usize)> = None;
        for (t, i) in tokens.iter().zip(first) {
            if let Some(i) = *i {
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
    /// that hits. `bodies` is the automaton over `tokens`, in their order.
    /// With `named`, a token naming this page's file is a hit that bumps
    /// `named[i]` and goes to `Candidate::file_tokens`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn candidate<'a>(
        &self,
        e: &'a Entry,
        tokens: &[String],
        query_words: &BTreeSet<String>,
        dates: &[QueryDate],
        df: &mut [usize],
        bodies: &AhoCorasick,
        mut named: Option<&mut [usize]>,
    ) -> (usize, Option<Candidate<'a>>) {
        let title = e.title.to_lowercase();
        let title_field = Field::new(&title);
        // The query is this page's whole title, in any order.
        let whole_title = *query_words == title_field.words.iter().cloned().collect();
        let alias_fields: Vec<Field> = e
            .aliases
            .iter()
            .map(|a| Field::new(&a.to_lowercase()))
            .collect();
        let summary_field: Option<Field> =
            e.summary.as_deref().map(|s| Field::new(&s.to_lowercase()));
        let symbols = self.symbol_fields.get(&e.id);
        // The query's words are one item's name words, in any order.
        let whole_symbol = symbols.is_some_and(|f| f.name_sets.contains(query_words));
        // Two or more query words, all inside one longer item name.
        let in_one_name = query_words.len() > 1
            && !whole_symbol
            && symbols.is_some_and(|f| f.name_sets.iter().any(|n| query_words.is_subset(n)));
        let mut sections = crate::rewrite::split_headings(self.body(&e.id), false);
        if e.kind.starts_with("code") {
            if let Some(body) = sections.get_mut("Body") {
                *body = unfence(body).to_string();
            }
        }
        let headings: Vec<String> = sections
            .keys()
            .filter(|k| {
                !Self::CHROME_SECTIONS.contains(&k.as_str())
                    && !Self::GENERATED_HEADINGS.contains(&k.as_str())
            })
            .map(|k| k.to_lowercase())
            .collect();
        let heading_fields: Vec<Field> = headings.iter().map(|h| Field::new(h)).collect();
        let content = Self::content_text(&sections);
        let content_lower = content.to_lowercase();
        let len = content.split_whitespace().count();

        let (body_tf, body_first) = Self::body_counts(bodies, &content_lower, tokens.len());
        let mut per_token = Vec::with_capacity(tokens.len());
        let mut file_tokens = Vec::new();
        let mut matched = false;
        let mut any_body = false;
        for (i, t) in tokens.iter().enumerate() {
            let t = t.as_str();
            let mut weight = 0.0;
            let title_tier = Self::field_tier(&title_field, t);
            let title_hit = title_tier > 0.0;
            weight += Self::W_NAME * title_tier;
            if whole_title {
                weight += Self::W_TITLE;
            }
            // A query spelling out an item's name (`get bin stock` for
            // `getBinStock`) scores each word `W_SYMBOL`, unless the title
            // already credits it: else `bin planner` put `planner.rs`
            // (`BinPlanner`) over the page titled `Bin Planner`. On the
            // 2,746-page project's 300 name-word queries file@1 went 196 →
            // 289, title queries 199/200 → 198.
            if whole_symbol && !title_hit {
                weight += Self::W_SYMBOL;
            }
            // Words that sit together in one item name (`order reject` in
            // `onOrderReject`) beat words scattered over several names
            // (`synth_reject`, `spawn_parcel_drain`). At 2.0 it cost exact
            // spellings their rank (`scene object` for `SceneObject`); 1.0
            // moved 167 → 177 of 184 such queries over 15 local repos and
            // no other eval row.
            if in_one_name && !title_hit {
                weight += Self::W_IN_ONE_NAME;
            }
            weight += Self::W_ALIAS * Self::best_tier(&alias_fields, t);
            // Whole-term equality, never `contains`: `to` must not hit
            // `token_estimate`, `wiki` must not hit `WikiError`.
            if let Some(f) = symbols {
                if f.whole.contains(t) {
                    weight += Self::W_SYMBOL;
                } else if f.words.contains(t) {
                    weight += Self::W_SYMBOL_WORD;
                } else if Self::with_query_stem(t, |st| f.word_stems.contains(st)) {
                    weight += Self::W_SYMBOL_WORD * Self::STEM_TIER;
                }
                let doc = Self::field_tier(&f.doc, t);
                if doc > 0.0 {
                    weight += Self::W_SYMBOL_DOC * doc;
                } else {
                    weight += Self::W_SYMBOL_DOC_REST * Self::field_tier(&f.doc_rest, t);
                }
            }
            weight += Self::W_SUMMARY
                * summary_field
                    .as_ref()
                    .map_or(0.0, |w| Self::field_tier(w, t));
            weight += Self::W_HEADING * Self::best_tier(&heading_fields, t);
            // Non-overlapping occurrences, as `match_indices` counts — the
            // spec'd counting rule. `tf` may exceed `len` for short tokens
            // ("on" in "one", "long"); `tf'` saturates, so that is harmless.
            let tf = body_tf[i];
            any_body |= tf > 0;
            let names_file = !e.memory && Self::names_file(&e.path, t);
            if let Some(named) = named.as_deref_mut().filter(|_| names_file) {
                named[i] += 1;
                file_tokens.push(i);
            }
            if weight > 0.0 || tf > 0 || file_tokens.last() == Some(&i) {
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
            Self::snippet(&content, tokens, &body_first)
        } else {
            None
        };
        let candidate = Candidate {
            entry: e,
            per_token,
            file_tokens,
            len,
            snippet,
        };
        (len, Some(candidate))
    }
}

// A whole-name match must outrank any word match, on the page and on the
// line, even when the word's side also matches by doc; a title match must
// outrank a whole-name match; words inside a longer name rank below both.
const _: () = assert!(
    Wiki::STEM_TIER > 0.0
        && Wiki::STEM_TIER < 1.0
        && Wiki::W_IN_ONE_NAME < Wiki::W_SYMBOL
        && Wiki::W_SYMBOL < Wiki::W_NAME
        && Wiki::W_SYMBOL > Wiki::W_SYMBOL_WORD + Wiki::W_SYMBOL_DOC
        && Wiki::W_SYMBOL_WORD > Wiki::W_SYMBOL_DOC
        && Wiki::IW_NAME_WORD > Wiki::IW_DOC
        && Wiki::IW_WHOLE > Wiki::IW_NAME_WORD + Wiki::IW_DOC
);
