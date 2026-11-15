//! Ranked search: `search` and its tagged, scoped and prompt forms, lifting and expansion.

use super::{
    gate_len, query_dates, unfence, Candidate, Entry, Hit, MemoryScope, PassStats, SymbolLine,
    Wiki, MEMORY_GATE_RATIO, MEMORY_TAIL,
};
use crate::model::{CodeItem, CodeItemKind, KindFilter};
use aho_corasick::AhoCorasick;
use std::collections::{BTreeSet, HashMap};

impl Wiki {
    /// A line is shown only if it scores at least this share of the best line
    /// in the whole response, or is (tied for) its page's best.
    const MIN_REL_SYMBOL: f64 = 0.3;

    /// Case-insensitive tokenized search over name/alias/item names/summary/section headings/body.
    /// Partial matching: a page is a hit if any token matches
    /// any field.
    /// Name, alias, summary and heading match when every alphanumeric part of the token is a prefix of one of the field's words (`field_hit`); body by substring; item names by whole term (the lowercased name or one of its words, see `symbol_fields`).
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
    /// (`symbol_fields`, weights `W_SYMBOL`, `W_SYMBOL_WORD`,
    /// `W_SYMBOL_DOC`),
    /// and each returned code page carries its best items as `symbols`
    /// (`expand`), with the same `idf`.
    ///
    /// A query word that is a page's file name or a path suffix of it
    /// (`names_file`) adds `W_FILE` on that page, see the constant; never on a
    /// memory note, and not in `search_prompt`.
    pub fn search(&self, q: &str, kind: Option<&KindFilter>, limit: usize) -> Vec<Hit> {
        self.search_tagged(q, kind, None, Some(limit))
    }

    /// `search` narrowed to pages tagged `tag` (already normalized by
    /// `tag_filter`) or a sub-tag of it; the tag check sits beside `kind`, so
    /// it narrows the corpus statistics too. With no query tokens and a tag,
    /// lists every page that passes both filters by path, score 0 — `limit`
    /// truncates only when given; with a query the default is 10.
    ///
    /// With a query the hits are ranked as if no memory notes existed, then
    /// `memory_tail` appends up to `MEMORY_TAIL` notes (not counted in
    /// `limit`, whatever `kind`).
    pub fn search_tagged(
        &self,
        q: &str,
        kind: Option<&KindFilter>,
        tag: Option<&str>,
        limit: Option<usize>,
    ) -> Vec<Hit> {
        if Self::tokenize(q).is_empty() {
            return self
                .search_terms(q, kind, tag, limit, MemoryScope::With, None, None)
                .0;
        }
        let (mut hits, stats) =
            self.search_terms(q, kind, tag, limit, MemoryScope::Without, None, None);
        // The ranked best, before `lift_names` may put an unscored page first.
        let best = hits.first().map_or(0.0, |h| h.score);
        self.lift_names(&mut hits, q, kind, tag, limit.unwrap_or(10));
        // The tail counts pages of every kind, so the code pass's statistics
        // are its own only when `kind` narrowed nothing.
        hits.extend(self.memory_tail(q, tag, best, kind.is_none().then_some(&stats)));
        hits
    }

    /// Most definitions of one name that `lift_names` lists.
    const DEFINITIONS_PER_NAME: usize = 3;

    /// A query word as an identifier: surrounding punctuation trimmed but a
    /// leading or trailing `_` kept (`_ack`, `run_`, `f()` gives `f`), and
    /// `None` unless it reads as one: it has `_` or `::`, or an upper-case
    /// letter after the first. Plain words (`order`, `Order`) stay with the
    /// ranked search, where a name like `order` is too common to put first.
    fn identifier(word: &str) -> Option<&str> {
        let t = word.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
        let well_formed = !t.starts_with(|c: char| c.is_ascii_digit())
            && t.split("::")
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_'));
        let named =
            t.contains('_') || t.contains("::") || t.chars().skip(1).any(char::is_uppercase);
        (well_formed && named).then_some(t)
    }

    /// The identifiers of `q` in order, deduplicated, and whether `q` is
    /// made of them alone.
    fn query_names(q: &str) -> (Vec<&str>, bool) {
        let words: Vec<&str> = q
            .split_whitespace()
            .filter(|w| w.chars().any(char::is_alphanumeric))
            .collect();
        let mut names: Vec<&str> = Vec::new();
        for n in words.iter().filter_map(|w| Self::identifier(w)) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
        let only = !words.is_empty() && words.iter().all(|w| Self::identifier(w).is_some());
        (names, only)
    }

    /// The items called `name` (or `owner::name`) on pages that pass `kind`
    /// and `tag`, matched case-sensitively: a name in another case is another
    /// item (`max_weekly_fee_usd` is a field, `MAX_WEEKLY_FEE_USD` a
    /// constant). Module and top-level items never.
    fn definitions(
        &self,
        name: &str,
        kind: Option<&KindFilter>,
        tag: Option<&str>,
    ) -> Vec<(&Entry, &CodeItem)> {
        let qualified = name.contains("::");
        self.items
            .iter()
            .filter_map(|(id, items)| Some((self.entries.get(id)?, items)))
            .filter(|(e, _)| e.passes(kind, tag))
            .flat_map(|(e, items)| items.iter().map(move |i| (e, i)))
            .filter(|(_, i)| {
                !matches!(i.kind, CodeItemKind::Module | CodeItemKind::TopLevel)
                    && if qualified {
                        i.qualified_name() == name
                    } else {
                        i.name == name
                    }
            })
            .collect()
    }

    /// The identifiers of a query made of identifiers alone (`query_names`)
    /// that name no item on a page passing `kind` and `tag`; empty for any
    /// other query.
    pub fn not_found(&self, q: &str, kind: Option<&KindFilter>, tag: Option<&str>) -> Vec<String> {
        let (names, only) = Self::query_names(q);
        if !only {
            return Vec::new();
        }
        names
            .into_iter()
            .filter(|n| {
                self.definitions(n, kind, tag).is_empty()
                    && self.stem_pages(n, kind, tag).is_empty()
            })
            .map(str::to_string)
            .collect()
    }

    /// The pages passing `kind` and `tag` whose file is known by `name`
    /// (`module_stem`, any case): `auto_refill` names `auto_refill.rs`, which
    /// is no item. Never a memory note.
    fn stem_pages(&self, name: &str, kind: Option<&KindFilter>, tag: Option<&str>) -> Vec<&Entry> {
        let name = name.to_lowercase();
        self.entries
            .values()
            .filter(|e| !e.memory && e.passes(kind, tag))
            .filter(|e| crate::formats::code::module_stem(&e.path).to_lowercase() == name)
            .collect()
    }

    /// Shows the definitions of `q`'s identifiers (`query_names`): per name
    /// in query order, its definitions (at most `DEFINITIONS_PER_NAME` and
    /// `limit`, pages ranked higher in `hits` first, then by pagerank), after
    /// the pages a name is the stem of when the query is of names alone
    /// (`stem_pages`). A query of
    /// identifiers alone, when any is defined, gets just those lines, page by
    /// page in the order the names first land, past the per-page line cap.
    /// In any other query a definition the ranking missed is added after its
    /// page's lines (the page after the hits), and nothing moves: on the
    /// regression set, putting a named line first cost top-1 answers where
    /// the name (`RejectCode`, `limits_for`) was context, not the target.
    fn lift_names(
        &self,
        hits: &mut Vec<Hit>,
        q: &str,
        kind: Option<&KindFilter>,
        tag: Option<&str>,
        limit: usize,
    ) {
        let (names, only) = Self::query_names(q);
        // Per page, in the order names first land on it: its definition lines.
        let mut by_page: Vec<(String, Vec<SymbolLine>)> = Vec::new();
        for name in names {
            let rank = |e: &Entry| hits.iter().position(|h| h.id == e.id).unwrap_or(usize::MAX);
            // In a query of names, a file the name is the stem of comes
            // first, as a page line: its module is no item.
            let mut stems = if only {
                self.stem_pages(name, kind, tag)
            } else {
                Vec::new()
            };
            stems.sort_by(|a, b| {
                rank(a)
                    .cmp(&rank(b))
                    .then_with(|| b.pagerank.total_cmp(&a.pagerank))
                    .then_with(|| a.path.cmp(&b.path))
            });
            stems.truncate(Self::DEFINITIONS_PER_NAME.min(limit));
            for e in stems {
                if !hits.iter().any(|h| h.id == e.id) {
                    hits.push(Hit::from_entry(e, 0.0, None));
                }
                if !by_page.iter().any(|(id, _)| *id == e.id) {
                    by_page.push((e.id.clone(), Vec::new()));
                }
            }
            let mut defs = self.definitions(name, kind, tag);
            let rank = |e: &Entry| hits.iter().position(|h| h.id == e.id).unwrap_or(usize::MAX);
            defs.sort_by(|(a, x), (b, y)| {
                rank(a)
                    .cmp(&rank(b))
                    .then_with(|| b.pagerank.total_cmp(&a.pagerank))
                    .then_with(|| a.path.cmp(&b.path))
                    .then_with(|| x.start_line.cmp(&y.start_line))
            });
            defs.truncate(Self::DEFINITIONS_PER_NAME.min(limit));
            for (e, i) in defs {
                let mut line = self.symbol_line(&e.path, i);
                if only && matches!(i.kind, CodeItemKind::Fn | CodeItemKind::Method) && !i.nested {
                    let n = self.caller_count(&e.path, i);
                    line.called_by = (n > 0).then_some(n);
                }
                let same = |s: &SymbolLine| {
                    (s.start_line, s.end_line, &s.name)
                        == (line.start_line, line.end_line, &line.name)
                };
                let at = hits.iter().position(|h| h.id == e.id).unwrap_or_else(|| {
                    hits.push(Hit::from_entry(e, 0.0, None));
                    hits.len() - 1
                });
                if !hits[at].symbols.iter().any(same) {
                    hits[at].symbols.push(line.clone());
                }
                match by_page.iter_mut().find(|(id, _)| *id == e.id) {
                    Some((_, lines)) if lines.iter().any(same) => {}
                    Some((_, lines)) => lines.push(line),
                    None => by_page.push((e.id.clone(), vec![line])),
                }
            }
        }
        if only && !by_page.is_empty() {
            let mut by_id: HashMap<String, Hit> =
                hits.drain(..).map(|h| (h.id.clone(), h)).collect();
            for (id, lines) in by_page {
                let mut hit = by_id.remove(&id).expect("a named page is a hit");
                hit.symbols = lines;
                hits.push(hit);
            }
        }
    }

    /// Whether note hit `h` may be shown for `q`: it is not superseded, and a
    /// query word of at least `n` characters names it (`names_page`), or `q`
    /// writes its date (`dated_on`), or its own score is positive and at
    /// least `MEMORY_GATE_RATIO` times `best_code`.
    pub(crate) fn passes_gate(&self, h: &Hit, q: &str, best_code: f64, n: usize) -> bool {
        h.superseded_by.is_none()
            && (self.names_page(&h.id, q, n)
                || self.dated_on(&h.id, q)
                || (h.score > 0.0 && h.score >= MEMORY_GATE_RATIO * best_code))
    }

    /// The memory notes `search` lists after its code hits: of the best ten
    /// notes for `q` (narrowed to `tag`) that pass the gate at `gate_len`,
    /// the most trusted (`Origin::trust`), then by score, the later
    /// `updated` first on a tie (dated before undated), at most
    /// `MEMORY_TAIL`. Empty when `q` has no word. `project`: the statistics
    /// of a `Without` pass for `q` and `tag` with no kind, which spares
    /// counting the project pages again.
    pub(crate) fn memory_tail(
        &self,
        q: &str,
        tag: Option<&str>,
        best_code: f64,
        project: Option<&PassStats>,
    ) -> Vec<Hit> {
        if !q.chars().any(char::is_alphanumeric) {
            return Vec::new();
        }
        let n = gate_len();
        let mut notes: Vec<Hit> = self
            .search_terms(q, None, tag, Some(10), MemoryScope::Only, None, project)
            .0
            .into_iter()
            .filter(|h| self.passes_gate(h, q, best_code, n))
            .collect();
        notes.sort_by(|a, b| b.origin.trust().total_cmp(&a.origin.trust()));
        notes.truncate(MEMORY_TAIL);
        notes
    }

    /// Code pages (or, with `MemoryScope::Only`, memory notes) for a prompt
    /// rather than a query: vendored pages are out, and only its `terms`
    /// rarest words that occur at all count, since most of a prompt's words
    /// say how to work, not what to find. `hint` scores its note with the
    /// same call as its code, so the gate's ratio compares like with like.
    /// Also returns `PassStats::available` for the prompt.
    pub(crate) fn search_prompt(
        &self,
        q: &str,
        limit: usize,
        terms: usize,
        scope: MemoryScope,
    ) -> (Vec<Hit>, f64, Vec<String>) {
        let (hits, stats) = self.search_terms(
            q,
            Some(&KindFilter::Code),
            None,
            Some(limit),
            scope,
            Some(terms),
            None,
        );
        (hits, stats.available, stats.terms)
    }

    /// How many of `terms` the item at lines `start..=end` of code page `id`
    /// (its `## Body`, plus the comment block right above `start`) matches:
    /// a term matches as a whole identifier, as a name word of one, or when
    /// every part of it shares a stem with such a name word.
    pub(crate) fn item_term_score(
        &self,
        id: &str,
        start: usize,
        end: usize,
        terms: &[String],
    ) -> usize {
        let sections = crate::rewrite::split_headings(self.body(id), false);
        let Some(body) = sections.get("Body") else {
            return 0;
        };
        let lines: Vec<&str> = unfence(body).lines().collect();
        if start == 0 || start > lines.len() {
            return 0;
        }
        let mut top = start - 1;
        while top > 0 {
            let t = lines[top - 1].trim_start();
            let comment = ["//", "#", "/*", "*", "--", ";", "\"\"\"", "'''"]
                .iter()
                .any(|m| t.starts_with(m));
            if !comment || t.starts_with("#include") || t.starts_with("#!") {
                break;
            }
            top -= 1;
        }
        let text = lines[top..end.min(lines.len())].join("\n");
        let lower = text.to_lowercase();
        let idents: BTreeSet<&str> = text
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty())
            .collect();
        let whole: BTreeSet<String> = idents
            .iter()
            .map(|w| w.trim_matches('_').to_lowercase())
            .collect();
        let words: BTreeSet<String> = idents.iter().flat_map(|w| Self::name_words(w)).collect();
        let stems: BTreeSet<String> = words.iter().map(|w| Self::stem(w)).collect();
        terms
            .iter()
            .filter(|t| {
                let punct = t.chars().any(|c| !(c.is_alphanumeric() || c == '_'));
                whole.contains(*t)
                    || (punct && lower.contains(t.as_str()))
                    || words.contains(*t)
                    || t.split(|c: char| !c.is_alphanumeric())
                        .filter(|p| !p.is_empty())
                        .all(|p| Self::with_query_stem(p, |sp| stems.contains(sp)))
            })
            .count()
    }

    /// Hits for `q` and the pass-1 statistics they were scored with: the
    /// memory notes only, all but them, or both (`scope`). A memory note
    /// passes the `code` and `docs` groups whatever its real kind (agents
    /// filter by `code`); an exact kind matches its real kind. With
    /// `MemoryScope::Only`, of two equal-scored notes the later `updated`
    /// comes first (dated before undated). With
    /// `prior` (the project pages' statistics of a `Without` pass for the
    /// same `q`, `tag` and no kind), an `Only` pass counts just the notes.
    #[allow(clippy::too_many_arguments)]
    fn search_terms(
        &self,
        q: &str,
        kind: Option<&KindFilter>,
        tag: Option<&str>,
        limit: Option<usize>,
        scope: MemoryScope,
        prompt_terms: Option<usize>,
        prior: Option<&PassStats>,
    ) -> (Vec<Hit>, PassStats) {
        let tokens = if prompt_terms.is_some() {
            Self::prompt_tokens(q)
        } else {
            Self::tokenize(q)
        };
        let query_words: BTreeSet<String> =
            tokens.iter().flat_map(|t| Self::field_words(t)).collect();
        let dates = query_dates(q);
        let in_scope = |e: &Entry| match scope {
            MemoryScope::With => true,
            MemoryScope::Without => !e.memory,
            MemoryScope::Only => e.memory,
        };
        let filtered = |e: &Entry| e.passes(kind, tag);
        let passes = |e: &Entry| in_scope(e) && filtered(e);
        if tokens.is_empty() {
            let Some(_) = tag else {
                return (Vec::new(), PassStats::default());
            };
            let mut listed: Vec<&Entry> = self.entries.values().filter(|e| passes(e)).collect();
            listed.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.id.cmp(&b.id)));
            listed.truncate(limit.unwrap_or(usize::MAX));
            let listed = listed.into_iter().map(|e| Hit::from_entry(e, 0.0, None));
            return (listed.collect(), PassStats::default());
        }
        let limit = limit.unwrap_or(10);

        // Pass 1: facts per page, statistics over every filtered page. Notes
        // alone are scored on the project's scale (statistics over project
        // pages plus notes), so the tail's ratio to the best code hit does
        // not shrink as the project grows; `Without` keeps notes out of them.
        // With `prior` the project pages are already counted.
        let counted = |e: &Entry| {
            if scope == MemoryScope::Only && prior.is_none() {
                filtered(e)
            } else {
                passes(e)
            }
        };
        let mut stats = prior.cloned().unwrap_or_else(|| PassStats {
            df: vec![0; tokens.len() + dates.len()],
            named: vec![0; tokens.len()],
            ..PassStats::default()
        });
        // One automaton finds every token in a body in one pass: per token
        // `match_indices` took 5 ms per prompt word on a 3,300-page wiki
        // (a 1,246-word hand-back: 4.5 s), and search itself 360 ms.
        let bodies = AhoCorasick::new(&tokens).expect("a query's tokens fit an automaton");
        let mut candidates: Vec<Candidate> = Vec::new();
        for e in self.entries.values() {
            if !counted(e) {
                continue;
            }
            // Not for a prompt: it names files in passing, and a hint has five
            // pages. Measured 2026-11-01 at `W_FILE`: of 143 prompts naming six
            // files the answer stayed in 52, against 119 without the rule.
            let (len, candidate) = self.candidate(
                e,
                &tokens,
                &query_words,
                &dates,
                &mut stats.df,
                &bodies,
                prompt_terms.is_none().then_some(&mut stats.named[..]),
            );
            // Statistics count every filtered page, hits or not — `avglen`
            // over hits alone would change with the query.
            stats.n += 1;
            stats.total_len += len;
            if in_scope(e) {
                candidates.extend(candidate);
            }
        }
        if candidates.is_empty() {
            return (Vec::new(), stats);
        }
        let PassStats {
            n,
            ref df,
            ref named,
            total_len,
            ..
        } = stats;

        // Pass 2: score. `n > 0` here because a candidate was counted.
        let n_f = n as f64;
        let avglen = total_len as f64 / n_f;
        // `avglen == 0` needs every filtered page's body to be missing on
        // disk; guard once here rather than divide by zero per page.
        let avglen = if avglen > 0.0 { avglen } else { 1.0 };
        let idf_of = |d: usize| (1.0 + (n_f - d as f64 + 0.5) / (d as f64 + 0.5)).ln();
        let mut idf: Vec<f64> = df.iter().map(|&d| idf_of(d)).collect();
        if let Some(terms) = prompt_terms {
            let mut seen: Vec<usize> = (0..tokens.len()).filter(|&i| df[i] > 0).collect();
            seen.sort_by_key(|&i| df[i]);
            for &i in seen.iter().skip(terms) {
                idf[i] = 0.0;
            }
            stats.available = seen.iter().take(terms).map(|&i| idf[i]).sum();
            stats.terms = seen
                .iter()
                .take(terms)
                .map(|&i| tokens[i].clone())
                .collect();
        }
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
                // As rare as the pages it names: `df` also counts every page
                // that includes the file, and that must not sink a header.
                let file: f64 = c
                    .file_tokens
                    .iter()
                    .map(|&i| idf_of(named[i]) / (named[i] as f64).sqrt())
                    .sum();
                let hit = Hit::from_entry(c.entry, base + Self::W_FILE * file, c.snippet);
                (hit, c.entry.pagerank)
            })
            .collect();
        ranked.sort_by(|(a, a_pr), (b, b_pr)| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| {
                    // Notes alone: newest first; never touches the code list.
                    if scope == MemoryScope::Only {
                        b.updated.cmp(&a.updated)
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then_with(|| b_pr.total_cmp(a_pr))
                .then_with(|| a.id.cmp(&b.id))
        });
        if prompt_terms.is_some() {
            ranked.retain(|(h, _)| h.score > 0.0);
        }
        ranked.truncate(limit);
        let mut hits: Vec<Hit> = ranked.into_iter().map(|(hit, _)| hit).collect();
        self.expand(&mut hits, &tokens, &query_words, &idf[..tokens.len()]);
        (hits, stats)
    }

    /// Gives each code hit its symbol lines (`page_symbols`), keeping only
    /// lines at or above `MIN_REL_SYMBOL` of the response's best line, and
    /// always the page's own best, so a page with any matching item keeps one.
    fn expand(
        &self,
        hits: &mut [Hit],
        tokens: &[String],
        query_words: &BTreeSet<String>,
        idf: &[f64],
    ) {
        let per_hit: Vec<Vec<(f64, &CodeItem)>> = hits
            .iter()
            .map(|h| {
                if self.is_code(&h.id) {
                    self.page_symbols(&h.id, tokens, query_words, idf)
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
            let own = lines.first().map_or(0.0, |(s, _)| *s);
            hit.symbols = lines
                .into_iter()
                .filter(|(s, _)| *s >= own || *s >= Self::MIN_REL_SYMBOL * best)
                .map(|(_, i)| {
                    let mut line = self.symbol_line(&hit.path, i);
                    line.doc = line.doc.filter(|d| docs.insert(d.clone()));
                    line
                })
                .collect();
        }
    }
}
