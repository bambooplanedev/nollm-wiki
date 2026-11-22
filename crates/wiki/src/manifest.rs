//! Manifest and index rendering: the page index (JSON and markdown), the llms-txt and AGENTS files, the graph export, and the per-page token estimate.

use crate::model::{clip, CodeItem, Edges, Entity, Graph, Origin, LINE_CHARS};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize)]
pub struct ManifestEntry {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub path: String,
    pub page: String,
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub memory: bool,
    /// `repo` or `local` on a memory note; absent on every other page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<&'static str>,
    /// The author name of the last commit to a `repo` note, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// A `repo` note that differs from `HEAD`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub edited: bool,
    /// A `repo` note whose author is the person who compiled.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub mine: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    pub degree_in: usize,
    pub degree_out: usize,
    pub pagerank: f64,
    pub token_estimate: usize,
    pub neighbors_out: Vec<String>,
    pub neighbors_in: Vec<String>,
    /// The import edges among `neighbors_out` (see `Edges::imports`).
    pub imports_out: Vec<String>,
    /// The import edges among `neighbors_in`.
    pub imports_in: Vec<String>,
    /// Top-level definition names of the page (see `Entity::defined`).
    pub defined: Vec<String>,
    /// Method names of the page (see `Entity::methods`). Declared last so
    /// it is the last key of every `index.json` entry: serde emits fields
    /// in declaration order.
    pub methods: Vec<String>,
}

#[derive(Serialize)]
pub struct Manifest {
    pub project: String,
    /// The compile input directory's own name (canonicalized, so `.` names
    /// it too): an absolute path resolves to a page only through it.
    pub root: String,
    pub entries: Vec<ManifestEntry>,
}

pub fn token_estimate(text: &str) -> usize {
    text.chars().count() / 4
}

/// The largest `n` in `0..=max` for which `fits(n)` holds, by bisection; `0` when none does.
/// `fits` must be monotone (true up to a point, false after) and is never asked about `0`.
pub(crate) fn largest_fit(max: usize, fits: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, max);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

pub fn build_manifest(
    project: &str,
    root: &str,
    entities: &BTreeMap<String, Entity>,
    graph: &Graph,
) -> Manifest {
    let entries = entities
        .iter()
        .map(|(id, e)| {
            let edges = graph.edges.get(id);
            let list = |f: fn(&Edges) -> &BTreeSet<String>| -> Vec<String> {
                edges
                    .map(|x| f(x).iter().cloned().collect())
                    .unwrap_or_default()
            };
            let (out, inc) = (list(|x| &x.outgoing), list(|x| &x.incoming));
            let (imports_out, imports_in) = (list(|x| &x.imports), list(|x| &x.imported_by));
            let (origin, author, edited, mine) = match &e.origin {
                _ if !e.memory => (None, None, false, false),
                Origin::Local => (Some("local"), None, false, false),
                Origin::Repo {
                    author,
                    edited,
                    mine,
                } => (Some("repo"), author.clone(), *edited, *mine),
            };
            ManifestEntry {
                id: id.clone(),
                title: e.name.clone(),
                kind: e.kind.label(),
                aliases: e.aliases.clone(),
                tags: e.tags.clone(),
                path: e.source_path.clone(),
                page: format!("{id}.md"),
                summary: e.summary.clone(),
                updated: Some(e.updated.clone()).filter(|u| !u.is_empty()),
                memory: e.memory,
                origin,
                author,
                edited,
                mine,
                superseded_by: e.superseded_by.clone(),
                degree_in: inc.len(),
                degree_out: out.len(),
                pagerank: *graph.pagerank.get(id).unwrap_or(&0.0),
                token_estimate: token_estimate(&e.body),
                neighbors_out: out,
                neighbors_in: inc,
                imports_out,
                imports_in,
                defined: e.defined.clone(),
                methods: e.methods.clone(),
            }
        })
        .collect();
    Manifest {
        project: project.to_string(),
        root: root.to_string(),
        entries,
    }
}

pub fn render_index_json(m: &Manifest) -> String {
    serde_json::to_string_pretty(m).expect("manifest serialization is infallible")
}

/// `items.json`: each code page's items (see `Entity::items`) keyed by page
/// id, in id order, pages without items left out. A sidecar rather than a
/// field of `index.json` entries, because items outweigh the rest of the
/// catalog several times over and `index.json` is read whole. Compact, as
/// only tools read it.
pub fn render_items_json(entities: &BTreeMap<String, Entity>) -> String {
    let items: BTreeMap<&str, &[CodeItem]> = entities
        .iter()
        .filter(|(_, e)| !e.items.is_empty())
        .map(|(id, e)| (id.as_str(), e.items.as_slice()))
        .collect();
    serde_json::to_string(&items).expect("items serialization is infallible")
}

pub fn render_index_md(m: &Manifest) -> String {
    let mut out = format!("# {} — Index\n\n", m.project);
    out.push_str(&format!("{} pages.\n\n", m.entries.len()));
    let by_path: BTreeMap<&str, &str> = m
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.id.as_str()))
        .collect();
    let ids: BTreeSet<&str> = m.entries.iter().map(|e| e.id.as_str()).collect();
    for e in &m.entries {
        let summary = crate::rewrite::obsidian_fixups(
            &summary_suffix(e),
            &e.path,
            &|id| ids.contains(id),
            &|p| by_path.get(p).map(ToString::to_string),
        );
        out.push_str(&format!(
            "- [{}]({}) — `{}` (in {}, out {}){}\n",
            e.title, e.page, e.kind, e.degree_in, e.degree_out, summary
        ));
    }
    out
}

/// `: <summary>` when the entry has one, nothing when it does not — so a
/// page without a summary never ends its index line in a dangling colon.
/// The summary is cut to one line of `LINE_CHARS` characters; a blank
/// summary counts as none.
fn summary_suffix(e: &ManifestEntry) -> String {
    e.summary
        .as_deref()
        .map(|s| clip(s, LINE_CHARS))
        .filter(|s| !s.is_empty())
        .map(|s| format!(": {s}"))
        .unwrap_or_default()
}

pub fn render_llms_txt(m: &Manifest) -> String {
    let mut out = format!("# {}\n\n", m.project);
    out.push_str(&format!(
        "> Generated wiki of {} pages. Start from this index; do not re-crawl the source.\n\n",
        m.entries.len()
    ));

    // Split by centrality: below-median pagerank goes under ## Optional.
    let mut ranks: Vec<f64> = m
        .entries
        .iter()
        .filter(|e| !e.memory)
        .map(|e| e.pagerank)
        .collect();
    ranks.sort_by(f64::total_cmp);
    let median = ranks.get(ranks.len() / 2).copied().unwrap_or(0.0);

    let line = |e: &ManifestEntry| format!("- [{}]({}){}\n", e.title, e.page, summary_suffix(e));
    out.push_str("## Docs\n\n");
    for e in m.entries.iter().filter(|e| e.pagerank >= median) {
        out.push_str(&line(e));
    }
    out.push_str("\n## Optional\n\n");
    for e in m.entries.iter().filter(|e| e.pagerank < median) {
        out.push_str(&line(e));
    }
    out
}

pub fn render_agents_md(project: &str) -> String {
    format!(
        "# Wiki navigation for agents\n\n\
This folder is a generated, cross-linked wiki of {project}. Do NOT re-read the source \
tree first — start here.\n\n\
- All pages: `index.md` (human) / `index.json` (machine catalog: id, title, kind, summary, degree, neighbors).\n\
- Machine map for tools: `llms.txt`.\n\
- Get a page + its N-hop neighborhood: `wiki neighbors <id> --depth N`.\n\
- Code symbols with their line ranges: `items.json` (keyed by page id).\n\
- Look up names you already know with `wiki search \"<name> <name>\"`; put several in one query. Use `wiki page <id>` only to see every symbol of a module you have not seen (one line per symbol, `start-end kind signature`); `--full` for the whole page.\n\
- What a function calls, through all levels: `wiki neighbors <id> --symbol <fn>`.\n\
- Do not call grep, find or ls in the same turn as `search`: read its answer first.\n\
- Where something is defined or handled: `search`. Who calls a function: `callers`. For every occurrence of a literal string (an env var, a message, a config key), grep.\n\
- Pages cross-link with `[[slug|Name]]` wikilinks (resolvable in Obsidian/Quartz and in `wiki lint`); each has Metadata / Related / Imported by / Mentioned in / Body sections.\n\
- Generated deterministically from source — do not hand-edit compiler-owned sections (the `## Notes` section is preserved).\n"
    )
}

pub fn render_graph_json(entities: &BTreeMap<String, Entity>, graph: &Graph) -> String {
    let nodes: Vec<serde_json::Value> = entities
        .iter()
        .map(|(id, e)| serde_json::json!({ "id": id, "title": e.name, "kind": e.kind.label(), "pagerank": graph.pagerank.get(id).unwrap_or(&0.0) }))
        .collect();
    let mut edges: Vec<serde_json::Value> = Vec::new();
    for (id, e) in &graph.edges {
        for target in &e.outgoing {
            edges.push(serde_json::json!({ "source": id, "target": target }));
        }
    }
    serde_json::to_string_pretty(&serde_json::json!({ "nodes": nodes, "edges": edges }))
        .expect("graph JSON serialization is infallible")
}

#[cfg(test)]
mod tests;
