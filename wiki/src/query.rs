use crate::model::SourceKind;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Clone)]
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
                .map(|p| p.to_lowercase().contains(&needle))
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
    /// `max_nodes` keeps the target plus the highest-centrality neighbors,
    /// dropping the lowest-centrality ones first. `max_tokens` caps the total
    /// token budget. Neighbors get summaries unless `full_neighbors` is set.
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

        // Neighbors (excluding target) ascending by pagerank → best lands last.
        let mut neighbor_ids: Vec<String> = seen.iter().filter(|n| *n != id).cloned().collect();
        neighbor_ids.sort_by(|a, b| {
            let pa = self.entries.get(a).map(|e| e.pagerank).unwrap_or(0.0);
            let pb = self.entries.get(b).map(|e| e.pagerank).unwrap_or(0.0);
            pa.total_cmp(&pb).then_with(|| a.cmp(b))
        });

        // Apply max_nodes: keep target + the highest-centrality neighbors (tail of the sorted list).
        if let Some(max) = budget.max_nodes {
            let keep = max.saturating_sub(1);
            if neighbor_ids.len() > keep {
                let drop = neighbor_ids.len() - keep;
                neighbor_ids.drain(0..drop); // drop lowest-centrality first
            }
        }

        // Build pack: target body first, then neighbor summaries (or full bodies).
        let mut text = String::new();
        let mut included = vec![id.to_string()];
        text.push_str(
            &self
                .page(id)
                .unwrap_or_else(|| format!("# {}\n", target.title)),
        );
        text.push_str("\n\n---\n\n");

        let mut tokens_used = target.token_estimate;
        for nid in &neighbor_ids {
            let e = match self.entries.get(nid) {
                Some(e) => e,
                None => continue,
            };
            if let Some(maxt) = budget.max_tokens {
                if tokens_used + e.token_estimate > maxt {
                    continue;
                }
            }
            if budget.full_neighbors {
                text.push_str(&self.page(nid).unwrap_or_default());
                tokens_used += e.token_estimate;
            } else {
                text.push_str(&format!(
                    "## {} ({})\n{}\n\n",
                    e.title,
                    nid,
                    e.summary.as_deref().unwrap_or("(no summary)")
                ));
                tokens_used += 20;
            }
            included.push(nid.clone());
        }

        Some(ContextPack { text, included })
    }
}
