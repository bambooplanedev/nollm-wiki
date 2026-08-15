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
}

#[derive(Deserialize)]
struct IndexFile {
    #[allow(dead_code)]
    project: String,
    entries: Vec<Entry>,
}

pub struct Hit {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub score: f64,
    /// Deterministic excerpt around the earliest body match (`None` for
    /// title/alias/summary-only hits — the summary explains those).
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

    /// All pages as `(id, title)` pairs, ascending by id (BTreeMap order).
    /// Used by the MCP server's `resources/list`.
    pub fn list_pages(&self) -> Vec<(String, String)> {
        self.entries
            .values()
            .map(|e| (e.id.clone(), e.title.clone()))
            .collect()
    }

    /// Sections excluded from search: generated chrome, so a query like
    /// "related" or "metadata" does not false-positive on every page.
    const CHROME_SECTIONS: [&'static str; 4] = ["Metadata", "Related", "Referenced By", "Notes"];

    // Field weights and the graded-occurrence bonus for search scoring.
    // Values from the 2026-07-14 search-quality design; tuning is a
    // constants-only change.
    const W_NAME: f64 = 3.0;
    const W_ALIAS: f64 = 2.0;
    const W_SUMMARY: f64 = 1.5;
    const W_BODY: f64 = 1.0;
    const W_OCCURRENCE: f64 = 0.1;
    const OCCURRENCE_CAP: usize = 20;

    /// The searchable *content* of a rendered page: every parsed section
    /// except the generated chrome (`CHROME_SECTIONS`). Subtractive on
    /// purpose — a doc body's own `## ` subheadings become sections of their
    /// own in `parse_sections`, and their text must stay searchable.
    ///
    /// Known residual limits (accepted by the 2026-07-14 search-quality
    /// design): content under an embedded heading named exactly like a
    /// chrome section stays unsearchable; duplicate heading names overwrite
    /// each other in the map; `parse_sections` also matches `## ` inside
    /// fenced code blocks (the text is still kept, under the example
    /// heading's name). Section order is BTreeMap (alphabetical), not
    /// document, order.
    fn content_text(page: &str) -> String {
        let sections = crate::rewrite::parse_sections(page);
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

    /// Case-insensitive tokenized search over name/alias/summary/body.
    /// AND semantics: every query token must match at least one field.
    /// Deterministic: per-token field weights + a capped occurrence bonus
    /// + pagerank tiebreak, sorted desc by score then asc by id, truncated
    ///   to `limit`. Empty/punctuation-only queries return no hits.
    pub fn search(&self, q: &str, kind: Option<SourceKind>, limit: usize) -> Vec<Hit> {
        let tokens = Self::tokenize(q);
        if tokens.is_empty() {
            return Vec::new();
        }
        let kind_label = kind.map(|k| k.label());
        let mut hits: Vec<Hit> = Vec::new();
        for e in self.entries.values() {
            if let Some(k) = &kind_label {
                if &e.kind != k {
                    continue;
                }
            }
            let title = e.title.to_lowercase();
            let aliases: Vec<String> = e.aliases.iter().map(|a| a.to_lowercase()).collect();
            let summary = e.summary.as_deref().map(str::to_lowercase);
            let content = self
                .page(&e.id)
                .map(|p| Self::content_text(&p))
                .unwrap_or_default();
            let content_lower = content.to_lowercase();

            let mut score = 0.0;
            let mut occurrences = 0usize;
            let mut all_match = true;
            let mut any_body = false;
            for t in &tokens {
                let name_hit = title.contains(t.as_str());
                let alias_hit = aliases.iter().any(|a| a.contains(t.as_str()));
                let summary_hit = summary
                    .as_deref()
                    .map(|s| s.contains(t.as_str()))
                    .unwrap_or(false);
                let token_occurrences = content_lower.match_indices(t.as_str()).count();
                let body_hit = token_occurrences > 0;
                any_body |= body_hit;
                if !(name_hit || alias_hit || summary_hit || body_hit) {
                    all_match = false;
                    break;
                }
                score += (name_hit as u8 as f64) * Self::W_NAME
                    + (alias_hit as u8 as f64) * Self::W_ALIAS
                    + (summary_hit as u8 as f64) * Self::W_SUMMARY
                    + (body_hit as u8 as f64) * Self::W_BODY;
                // match_indices is non-overlapping — the spec'd counting rule.
                occurrences += token_occurrences;
            }
            if !all_match {
                continue;
            }
            score += Self::W_OCCURRENCE * occurrences.min(Self::OCCURRENCE_CAP) as f64;
            score += e.pagerank;
            let snippet = if any_body {
                Self::snippet(&content, &content_lower, &tokens)
            } else {
                None
            };
            hits.push(Hit {
                id: e.id.clone(),
                title: e.title.clone(),
                summary: e.summary.clone(),
                score,
                snippet,
            });
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
        hits.truncate(limit);
        hits
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

    /// BFS to `depth` over neighbors_out+neighbors_in, then build a budgeted
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
    pub fn neighbors(&self, id: &str, depth: usize, budget: &PackBudget) -> Option<ContextPack> {
        let target = self.entries.get(id)?;

        let seen = self.bfs_neighborhood(id, depth);

        // Candidates (excluding target), descending by pagerank, so every
        // budget below always considers the highest-centrality neighbor
        // first.
        let mut candidates: Vec<String> = seen.iter().filter(|n| *n != id).cloned().collect();
        candidates.sort_by(|a, b| {
            let pa = self.entries.get(a).map(|e| e.pagerank).unwrap_or(0.0);
            let pb = self.entries.get(b).map(|e| e.pagerank).unwrap_or(0.0);
            pb.total_cmp(&pa).then_with(|| a.cmp(b))
        });

        // Apply max_nodes: keep target + the highest-centrality neighbors
        // (the head of the descending list); the lowest-centrality tail is
        // dropped first.
        if let Some(max) = budget.max_nodes {
            let keep = max.saturating_sub(1);
            candidates.truncate(keep);
        }

        const SEPARATOR: &str = "\n\n---\n\n";
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
        let sep_chars = SEPARATOR.chars().count();
        let target_block = if fits(full_target.chars().count() + sep_chars) {
            full_target
        } else {
            format!(
                "# {} ({id})\n\n{}\n\n_body (~{} tokens) exceeds the budget; read wiki://page/{id} for the full page_\n",
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
            let e = match self.entries.get(&nid) {
                Some(e) => e,
                None => continue,
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
            let pa = self.entries.get(a).map(|e| e.pagerank).unwrap_or(0.0);
            let pb = self.entries.get(b).map(|e| e.pagerank).unwrap_or(0.0);
            pa.total_cmp(&pb).then_with(|| a.cmp(b))
        });

        let mut text = String::new();
        let mut included = vec![id.to_string()];
        text.push_str(&target_block);
        text.push_str(SEPARATOR);
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
}
