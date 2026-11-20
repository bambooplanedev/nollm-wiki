//! The `Wiki` text pipeline: tokenizing, stemming and field-word matching.

use super::{Field, Wiki};
use std::collections::{BTreeMap, HashMap};

type StemMemo = std::sync::RwLock<HashMap<String, String>>;
/// Stems of corpus words (`Wiki::with_stem`).
pub(super) static CORPUS_STEMS: std::sync::LazyLock<StemMemo> =
    std::sync::LazyLock::new(Default::default);
/// Stems of query words (`Wiki::with_query_stem`).
pub(super) static QUERY_STEMS: std::sync::LazyLock<StemMemo> =
    std::sync::LazyLock::new(Default::default);
/// Most entries `QUERY_STEMS` holds; one search's words fit many times over.
pub(super) const QUERY_STEMS_MAX: usize = 4096;

impl Wiki {
    /// The searchable *content* of a parsed page: every section body except
    /// the generated chrome (`CHROME_SECTIONS`). Subtractive on purpose — a
    /// doc body's own `## ` subheadings become sections of their own in
    /// `split_headings`, and their text must stay searchable. The heading
    /// names themselves are scored separately (see `search`).
    ///
    /// Known residual limits (accepted by the 2026-07-14 search-quality
    /// design): content under an embedded heading named exactly like a
    /// chrome section stays unsearchable, and duplicate heading names
    /// overwrite each other in the map. Section order is `BTreeMap`
    /// (alphabetical), not document, order.
    pub(super) fn content_text(sections: &BTreeMap<String, String>) -> String {
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
    pub(crate) fn tokenize(query: &str) -> Vec<String> {
        let mut tokens: Vec<String> = Vec::new();
        for raw in query.to_lowercase().split_whitespace() {
            let t = raw.trim_matches(|c: char| !c.is_alphanumeric());
            if !t.is_empty() && !tokens.iter().any(|x| x == t) {
                tokens.push(t.to_string());
            }
        }
        tokens
    }

    /// Snowball stem of a token, part by part: each run of alphanumeric
    /// characters is stemmed on its own (Ukrainian for Cyrillic, English
    /// otherwise) and the separators between them stay, so `mcp-сервери`
    /// gives `mcp-сервер`. Lowercase input, as the stemmers expect.
    ///
    /// Memoised process-wide in `CORPUS_STEMS`: a search stems every field
    /// word of every page, and the same words recur across pages. Only
    /// corpus words go there (query words go to `with_query_stem`), so it
    /// holds the corpus vocabulary at most. Without it a hint on a
    /// 3,142-page wiki took 2.0 s against 0.96 s before stemming.
    pub(super) fn stem(token: &str) -> String {
        Self::with_stem(token, str::to_string)
    }

    /// `f` applied to the memoised stem of corpus word `token`, without
    /// copying it out.
    fn with_stem<T>(token: &str, f: impl FnOnce(&str) -> T) -> T {
        Self::stem_in(&CORPUS_STEMS, usize::MAX, token, f)
    }

    /// `with_stem` for a query word: a search stems it once per page, so it
    /// is memoised too, in `QUERY_STEMS`, which is emptied at
    /// `QUERY_STEMS_MAX` entries so a long-lived `serve` does not keep every
    /// word of every prompt.
    pub(super) fn with_query_stem<T>(token: &str, f: impl FnOnce(&str) -> T) -> T {
        Self::stem_in(&QUERY_STEMS, QUERY_STEMS_MAX, token, f)
    }

    fn stem_in<T>(memo: &StemMemo, cap: usize, token: &str, f: impl FnOnce(&str) -> T) -> T {
        if let Some(hit) = memo.read().unwrap().get(token) {
            return f(hit);
        }
        let stemmed = Self::stem_uncached(token);
        let out = f(&stemmed);
        let mut memo = memo.write().unwrap();
        if memo.len() >= cap {
            memo.clear();
        }
        memo.insert(token.to_string(), stemmed);
        out
    }

    fn stem_uncached(token: &str) -> String {
        let mut out = String::with_capacity(token.len());
        let mut part = String::new();
        let flush = |part: &mut String, out: &mut String| {
            if part.is_empty() {
                return;
            }
            let cyrillic = part.chars().any(|c| ('\u{400}'..='\u{4FF}').contains(&c));
            let algorithm = if cyrillic {
                frostem::Algorithm::Ukrainian
            } else {
                frostem::Algorithm::English
            };
            out.push_str(&frostem::Stemmer::new(algorithm).stem(part));
            part.clear();
        };
        for c in token.chars() {
            if c.is_alphanumeric() {
                part.push(c);
            } else {
                flush(&mut part, &mut out);
                out.push(c);
            }
        }
        flush(&mut part, &mut out);
        out
    }

    /// The tokens of a prompt that `search_prompt` scores: `tokenize` less
    /// the one-character ones. `в`, `і` and `y` alone found the one file
    /// holding them (a Cyrillic comment, a single-letter variable) and, as
    /// the rarest words of a Ukrainian prompt, steered every hint there.
    pub(crate) fn prompt_tokens(text: &str) -> Vec<String> {
        let mut tokens = Self::tokenize(text);
        tokens.retain(|t| t.chars().count() > 1);
        tokens
    }

    /// The words of an identifier, lowercased: `wiki_code::support::name_words`.
    pub(crate) fn name_words(name: &str) -> Vec<String> {
        wiki_code::support::name_words(name)
    }

    /// The words of a lowercased field: maximal runs of alphanumeric
    /// characters (`char::is_alphanumeric`, so digits count and `_`, `-`,
    /// `:` and punctuation split). Computed once per page per field.
    pub(super) fn field_words(field: &str) -> Vec<String> {
        field
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// `field_words` of a lowercased optional field; empty for `None`.
    pub(super) fn lower_words(field: Option<&str>) -> Vec<String> {
        field
            .map(|f| Self::field_words(&f.to_lowercase()))
            .unwrap_or_default()
    }

    /// Whether query token `t` is the file name of `path` (with its extension,
    /// any case) or a suffix of the path that starts at a folder. Field words
    /// cannot say this: they split `strategy_config.h` into parts, `h` is in no
    /// title, and the token then hits only the pages that include the file.
    pub(super) fn names_file(path: &str, t: &str) -> bool {
        let path = path.to_lowercase();
        t.rsplit('/').next().is_some_and(|name| name.contains('.'))
            && (path == t || path.strip_suffix(t).is_some_and(|r| r.ends_with('/')))
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
    pub(super) fn field_hit(words: &[String], t: &str) -> bool {
        debug_assert!(
            t.chars().any(char::is_alphanumeric),
            "tokenize never yields a token without an alphanumeric part"
        );
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|p| !p.is_empty())
            .all(|p| words.iter().any(|w| w.starts_with(p)))
    }

    /// How well query token `t` hits a field: 1.0 by `field_hit` (exact or
    /// prefix), `STEM_TIER` when every alphanumeric part of `t` shares its
    /// Snowball stem with a word of the field (`provisioning` ↔
    /// `provision`, `сторінок` ↔ `сторінка`), else 0.0.
    pub(super) fn field_tier(field: &Field, t: &str) -> f64 {
        if Self::field_hit(&field.words, t) {
            return 1.0;
        }
        let shares_stem = t
            .split(|c: char| !c.is_alphanumeric())
            .filter(|p| !p.is_empty())
            .all(|p| Self::with_query_stem(p, |sp| field.stems.iter().any(|s| s == sp)));
        if shares_stem {
            Self::STEM_TIER
        } else {
            0.0
        }
    }

    /// The best `field_tier` of `t` over several fields (aliases, headings).
    pub(super) fn best_tier(fields: &[Field], t: &str) -> f64 {
        fields
            .iter()
            .map(|w| Self::field_tier(w, t))
            .fold(0.0, f64::max)
    }

    /// Whether a whole word of `words` has the stem of `t`: the stem tier of
    /// the whole-term name-word match (`levels` ↔ `level`).
    pub(super) fn shares_stem<'a>(words: impl IntoIterator<Item = &'a String>, t: &str) -> bool {
        Self::with_query_stem(t, |st| {
            words.into_iter().any(|w| Self::with_stem(w, |sw| sw == st))
        })
    }
}
