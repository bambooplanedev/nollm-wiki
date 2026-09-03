//! Query surface over a compiled wiki: search, page lookup, neighbor packs.
//!
//! `leading_doc` takes only the first line of this comment as the page
//! summary, so that line has to stand on its own. Detail goes below it:
//! `neighbors` walks outward from a page and stops at a caller-supplied
//! token ceiling, degrading an oversized target to a title-and-summary block.

use crate::model::SourceKind;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Entry {
    id: String,
    title: String,
    kind: String,
    aliases: Vec<String>,
    summary: Option<String>,
    pagerank: f64,
    token_estimate: usize,
    neighbors_out: Vec<String>,
    neighbors_in: Vec<String>,
    /// Top-level definition names (`ManifestEntry::defined`). Defaulted so
    /// an `index.json` written by a compiler older than this field still
    /// loads: `serve` reads wikis it did not compile.
    #[serde(default)]
    defined: Vec<String>,
    /// Method names (`ManifestEntry::methods`). Defaulted for the same
    /// reason as `defined`: an older `index.json` has no such key.
    #[serde(default)]
    #[allow(dead_code)] // read by Task 4 of the 2026-09-03 plan
    methods: Vec<String>,
}

#[derive(Deserialize)]
struct IndexFile {
    entries: Vec<Entry>,
}

pub struct Hit {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub score: f64,
    /// Deterministic excerpt around the earliest body match (`None` for
    /// hits that matched only title, alias, defined name, summary, or a
    /// section heading — the summary explains those).
    pub snippet: Option<String>,
}

#[derive(Default)]
pub struct PackBudget {
    pub max_nodes: Option<usize>,
    pub max_tokens: Option<usize>,
    pub full_neighbors: bool,
}

pub struct ContextPack {
    pub text: String,
    pub included: Vec<String>,
}

/// What `Wiki::search` learns about one page in its first pass. Scores
/// need corpus-wide statistics (`df`, `avglen`), so per-page facts are
/// held until every page has been seen; the page text itself is not.
/// The phrase a degraded target block carries when a page's body does not
/// fit the pack's token budget. Tests assert on it; keep it in one place.
pub const OVER_BUDGET_NOTE: &str = "exceeds the budget";

struct Candidate<'a> {
    entry: &'a Entry,
    /// Per query token, in query order: (summed field weights, body
    /// occurrences).
    per_token: Vec<(f64, usize)>,
    /// Whitespace-word count of the searchable content.
    len: usize,
    snippet: Option<String>,
}

pub struct Wiki {
    dir: PathBuf,
    entries: BTreeMap<String, Entry>,
}

impl Wiki {
    /// Load a compiled output directory: reads `index.json` for metadata +
    /// adjacency; page bodies are read on demand from `<id>.md`.
    pub fn load(dir: &Path) -> Result<Wiki, std::io::Error> {
        let text = std::fs::read_to_string(dir.join("index.json"))?;
        let index: IndexFile = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let entries = index
            .entries
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();
        Ok(Wiki {
            dir: dir.to_path_buf(),
            entries,
        })
    }

    /// Read a page's rendered Markdown body by id.
    pub fn page(&self, id: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.join(format!("{id}.md"))).ok()
    }

    /// Whether `id` is a page in the loaded index. Used by the MCP server to
    /// validate resource ids before touching the filesystem.
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
    const CHROME_SECTIONS: [&'static str; 4] = ["Metadata", "Related", "Referenced By", "Notes"];

    /// Generated content headings excluded from the `heading` field: they
    /// sit on nearly every page and say nothing about it. Measured on the
    /// eval corpus, leaving them in made `exports` hit 17 of 17 pages.
    const GENERATED_HEADINGS: [&'static str; 3] = ["Body", "Exports", "Imports"];

    // Field weights for search scoring. Values from the 2026-09-02
    // search-scoring design; tuning is a constants-only change.
    // `W_HEADING` is deliberately below `W_SUMMARY`: at 1.5 an incidental
    // heading word on a large page outranked the page named for the query.
    const W_NAME: f64 = 3.0;
    const W_ALIAS: f64 = 2.0;
    // `W_DEFINED` (2026-09-02 defined-names design): equal to `W_ALIAS`,
    // above `W_SUMMARY`, below `W_NAME`, so a page whose title is the query
    // still beats a page that merely defines a same-named item. Measured on
    // the 547-query live set: own-title hits unchanged at 35/37, 26 flips
    // better / 6 worse; 1.5 moved fewer definer queries, 2.5 added a worse row.
    const W_DEFINED: f64 = 2.0;
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
            .map(|(_, v)| v.clone())
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

    /// The words of an identifier, lowercased: split on `_`/`-`, at a
    /// lower→Upper boundary, and at an Upper→Upper-lower boundary so an
    /// acronym run stays one word (`HTTPServer` → `http`, `server`;
    /// `PackBudget` → `pack`, `budget`; `CACHE_VERSION` → `cache`,
    /// `version`). Must run on the ORIGINAL-case name: lowercasing first
    /// erases every CamelCase boundary (the 2026-09-02 V7 prototype's dead
    /// rule).
    fn name_words(name: &str) -> Vec<String> {
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

    /// The search terms a page earns from its `defined` names: each
    /// lowercased full name plus each of its words, minus the self-name
    /// skip. The skip is word-level: a word equal to a title word is
    /// dropped (the title already scores it at `W_NAME`; crediting it again
    /// let `Cache` on page `cache` outrank `code`), the other words stay,
    /// and the full name stays unless it is itself a title word — the title
    /// never matches the full-name token, so dropping the whole name would
    /// leave the definer with no credit (`codeextractor` on `code`).
    fn defined_terms(title_lower: &str, defined: &[String]) -> BTreeSet<String> {
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
        terms
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
            .split_whitespace()
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
        df: &mut [usize],
    ) -> (usize, Option<Candidate<'a>>) {
        let title = e.title.to_lowercase();
        let title_words = Self::field_words(&title);
        let alias_words: Vec<Vec<String>> = e
            .aliases
            .iter()
            .map(|a| Self::field_words(&a.to_lowercase()))
            .collect();
        let summary_words: Option<Vec<String>> = e
            .summary
            .as_deref()
            .map(|s| Self::field_words(&s.to_lowercase()));
        let defined_terms = Self::defined_terms(&title, &e.defined);
        let page = self.page(&e.id).unwrap_or_default();
        let sections = crate::rewrite::parse_sections(&page);
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
            if alias_words.iter().any(|w| Self::field_hit(w, t)) {
                weight += Self::W_ALIAS;
            }
            // Whole-term equality, never `contains`: `to` must not hit
            // `token_estimate`, `wiki` must not hit `WikiError`.
            if defined_terms.contains(t) {
                weight += Self::W_DEFINED;
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
    pub fn search(&self, q: &str, kind: Option<SourceKind>, limit: usize) -> Vec<Hit> {
        let tokens = Self::tokenize(q);
        if tokens.is_empty() {
            return Vec::new();
        }
        let kind_label = kind.map(|k| k.label());

        // Pass 1: facts per page, statistics over every filtered page.
        let mut n = 0usize;
        let mut total_len = 0usize;
        let mut df = vec![0usize; tokens.len()];
        let mut candidates: Vec<Candidate> = Vec::new();
        for e in self.entries.values() {
            if let Some(k) = &kind_label {
                if &e.kind != k {
                    continue;
                }
            }
            let (len, candidate) = self.candidate(e, &tokens, &mut df);
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
                let score = c
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
                let hit = Hit {
                    id: c.entry.id.clone(),
                    title: c.entry.title.clone(),
                    summary: c.entry.summary.clone(),
                    score,
                    snippet: c.snippet,
                };
                (hit, c.entry.pagerank)
            })
            .collect();
        ranked.sort_by(|(a, a_pr), (b, b_pr)| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| b_pr.total_cmp(a_pr))
                .then_with(|| a.id.cmp(&b.id))
        });
        ranked.truncate(limit);
        ranked.into_iter().map(|(hit, _)| hit).collect()
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

    /// BFS to `depth` over `neighbors_out+neighbors_in`, then build a budgeted
    /// context pack: target first, neighbors ordered ascending by pagerank
    /// (highest-centrality lands last — "lost in the middle").
    ///
    /// `max_tokens` is a hard ceiling on the estimated size of the returned
    /// pack text (`manifest::token_estimate`, chars/4): every block is
    /// charged at the size of the text actually emitted. When the target's
    /// full page alone would blow the ceiling, it degrades to a summary
    /// block (title + summary + a pointer at `wiki://page/<id>`) so the
    /// neighborhood still fits. The one floor exception: the degraded block
    /// is always emitted, even when a pathologically small budget cannot
    /// contain it — a neighbors call always says something about the target.
    ///
    /// Neighbor selection walks candidates in descending centrality and
    /// keeps whichever fit the remaining budget (skip, not break), so the
    /// kept set is the highest-centrality set the greedy can admit —
    /// deliberately NOT a maximum-cardinality fill. A heavier high-
    /// centrality neighbor beats several lighter lower-centrality ones; see
    /// `full_neighbors_max_tokens_prefers_centrality_over_packing`.
    /// Neighbors get summary blocks unless `full_neighbors` is set.
    /// Between the blocks of a context pack.
    const SEPARATOR: &str = "\n\n---\n\n";

    /// Pagerank of `id`, or 0 for an id the index does not know.
    fn pagerank_of(&self, id: &str) -> f64 {
        self.entries.get(id).map_or(0.0, |e| e.pagerank)
    }

    pub fn neighbors(&self, id: &str, depth: usize, budget: &PackBudget) -> Option<ContextPack> {
        let target = self.entries.get(id)?;

        let seen = self.bfs_neighborhood(id, depth);

        // Candidates (excluding target), descending by pagerank, so every
        // budget below always considers the highest-centrality neighbor
        // first.
        let mut candidates: Vec<String> = seen.iter().filter(|n| *n != id).cloned().collect();
        candidates.sort_by(|a, b| {
            self.pagerank_of(b)
                .total_cmp(&self.pagerank_of(a))
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

        // Target block: the full page, degraded to a summary block when
        // the full page (plus separator) alone would blow the ceiling.
        let full_target = self
            .page(id)
            .unwrap_or_else(|| format!("# {}\n", target.title));
        let sep_chars = Self::SEPARATOR.chars().count();
        let target_block = if fits(full_target.chars().count() + sep_chars) {
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

        // Admit neighbors in descending centrality, each charged at the
        // size of the block actually emitted (summary line or full page).
        let mut kept: Vec<(String, String)> = Vec::new();
        for nid in candidates {
            let Some(e) = self.entries.get(&nid) else {
                continue;
            };
            let block = if budget.full_neighbors {
                self.page(&nid).unwrap_or_default()
            } else {
                format!(
                    "## {} ({})\n{}\n\n",
                    e.title,
                    nid,
                    e.summary.as_deref().unwrap_or("(no summary)")
                )
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

#[cfg(test)]
mod tests {
    use super::Wiki;
    use std::collections::BTreeSet;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(std::string::ToString::to_string).collect()
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
            Wiki::defined_terms("code", &["CodeExtractor".into(), "load".into()]),
            set(&["codeextractor", "extractor", "load"])
        );
        // A name that IS the title word contributes nothing.
        assert_eq!(Wiki::defined_terms("cache", &["Cache".into()]), set(&[]));
        // Multi-word title: every title word is skipped.
        assert_eq!(
            Wiki::defined_terms("graph page", &["build_graph".into(), "page_ids".into()]),
            set(&["build", "build_graph", "ids", "page_ids"])
        );
        assert_eq!(
            Wiki::defined_terms("hash", &["hash_bytes".into()]),
            set(&["bytes", "hash_bytes"])
        );
    }
}
