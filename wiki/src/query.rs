use crate::model::SourceKind;
use serde::Deserialize;
use std::collections::BTreeMap;
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

    /// All pages as `(id, title)` pairs, ascending by id (BTreeMap order).
    /// Used by the MCP server's `resources/list`.
    pub fn list_pages(&self) -> Vec<(String, String)> {
        self.entries
            .values()
            .map(|e| (e.id.clone(), e.title.clone()))
            .collect()
    }

    /// The searchable *content* of a rendered page: the Body, Exports, and
    /// Imports sections only. Excludes generated chrome (the banner, Metadata,
    /// Related, Referenced By, Notes) so a query like "related" or "metadata"
    /// does not false-positive on every page.
    fn content_text(page: &str) -> String {
        let sections = crate::rewrite::parse_sections(page);
        ["Body", "Exports", "Imports"]
            .iter()
            .filter_map(|k| sections.get(*k))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Case-insensitive search over name/alias/summary/body. Deterministic:
    /// field-weighted score with a pagerank tiebreak, sorted desc by score
    /// then asc by id, truncated to `limit`.
    pub fn search(&self, q: &str, kind: Option<SourceKind>, limit: usize) -> Vec<Hit> {
        let needle = q.to_lowercase();
        let kind_label = kind.map(|k| k.label());
        let mut hits: Vec<Hit> = Vec::new();
        for e in self.entries.values() {
            if let Some(k) = &kind_label {
                if &e.kind != k {
                    continue;
                }
            }
            let name_hit = e.title.to_lowercase().contains(&needle);
            let alias_hit = e.aliases.iter().any(|a| a.to_lowercase().contains(&needle));
            let summary_hit = e
                .summary
                .as_deref()
                .map(|s| s.to_lowercase().contains(&needle))
                .unwrap_or(false);
            let body_hit = self
                .page(&e.id)
                .map(|p| Self::content_text(&p).to_lowercase().contains(&needle))
                .unwrap_or(false);
            if !(name_hit || alias_hit || summary_hit || body_hit) {
                continue;
            }
            // Deterministic score: field weight + pagerank tiebreak.
            let score = (name_hit as u8 as f64) * 3.0
                + (alias_hit as u8 as f64) * 2.0
                + (summary_hit as u8 as f64) * 1.5
                + (body_hit as u8 as f64)
                + e.pagerank;
            hits.push(Hit {
                id: e.id.clone(),
                title: e.title.clone(),
                summary: e.summary.clone(),
                score,
            });
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
        hits.truncate(limit);
        hits
    }

    /// BFS to `depth` over neighbors_out+neighbors_in, then build a budgeted
    /// context pack: target first (full body), neighbors ordered ascending by
    /// pagerank (highest-centrality lands last — "lost in the middle").
    /// Both `max_nodes` and `max_tokens` keep the highest-centrality
    /// neighbors that fit, dropping the lowest-centrality ones first:
    /// selection always walks candidates in *descending* centrality order,
    /// and only the final emission re-sorts the kept set back to ascending.
    /// Neighbors get summaries unless `full_neighbors` is set. The target is
    /// always included, even if its own token estimate alone exceeds
    /// `max_tokens`.
    pub fn neighbors(&self, id: &str, depth: usize, budget: &PackBudget) -> Option<ContextPack> {
        let target = self.entries.get(id)?;

        // BFS collect neighbor ids up to `depth`.
        let mut seen = std::collections::BTreeSet::new();
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

        // Apply max_tokens: walk the (still descending) candidates and keep
        // whichever fit the running budget. A candidate that doesn't fit is
        // skipped with `continue` (not `break`) so a smaller, lower-
        // centrality neighbor further down the list can still use any
        // leftover budget — at every step the highest-centrality neighbor
        // that fits wins, so the kept set stays as high-centrality as the
        // budget allows. The target is added unconditionally up front, even
        // if its own token estimate alone already exceeds `max_tokens`.
        // Guarantee: candidates are visited in descending centrality and kept
        // if they fit the running budget, so the result is the highest-
        // centrality set the greedy can admit — deliberately NOT a maximum-
        // cardinality fill. A heavier high-centrality neighbor is preferred
        // over several lighter lower-centrality ones. See the query test
        // `full_neighbors_max_tokens_prefers_centrality_over_packing`.
        let mut tokens_used = target.token_estimate;
        let mut kept: Vec<String> = Vec::new();
        for nid in candidates {
            let e = match self.entries.get(&nid) {
                Some(e) => e,
                None => continue,
            };
            // Summary-mode cost is a flat approximation, not measured from
            // the actual rendered summary line.
            let cost = if budget.full_neighbors {
                e.token_estimate
            } else {
                20
            };
            if let Some(maxt) = budget.max_tokens {
                if tokens_used.saturating_add(cost) > maxt {
                    continue;
                }
            }
            tokens_used = tokens_used.saturating_add(cost);
            kept.push(nid);
        }

        // Emit ascending by pagerank so the highest-centrality neighbor
        // lands last ("lost in the middle" placement).
        kept.sort_by(|a, b| {
            let pa = self.entries.get(a).map(|e| e.pagerank).unwrap_or(0.0);
            let pb = self.entries.get(b).map(|e| e.pagerank).unwrap_or(0.0);
            pa.total_cmp(&pb).then_with(|| a.cmp(b))
        });

        // Build pack: target body first, then neighbor summaries (or full bodies).
        let mut text = String::new();
        let mut included = vec![id.to_string()];
        text.push_str(
            &self
                .page(id)
                .unwrap_or_else(|| format!("# {}\n", target.title)),
        );
        text.push_str("\n\n---\n\n");

        for nid in &kept {
            let e = match self.entries.get(nid) {
                Some(e) => e,
                None => continue,
            };
            if budget.full_neighbors {
                text.push_str(&self.page(nid).unwrap_or_default());
            } else {
                text.push_str(&format!(
                    "## {} ({})\n{}\n\n",
                    e.title,
                    nid,
                    e.summary.as_deref().unwrap_or("(no summary)")
                ));
            }
            included.push(nid.clone());
        }

        Some(ContextPack { text, included })
    }
}
