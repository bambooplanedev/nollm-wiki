//! Manifest and index rendering: the page index (JSON and markdown), the llms-txt and AGENTS files, the graph export, and the per-page token estimate.

use crate::model::{Entity, Graph};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct ManifestEntry {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub path: String,
    pub page: String,
    pub summary: Option<String>,
    pub degree_in: usize,
    pub degree_out: usize,
    pub pagerank: f64,
    pub token_estimate: usize,
    pub neighbors_out: Vec<String>,
    pub neighbors_in: Vec<String>,
}

#[derive(Serialize)]
pub struct Manifest {
    pub project: String,
    pub entries: Vec<ManifestEntry>,
}

pub fn token_estimate(text: &str) -> usize {
    text.chars().count() / 4
}

pub fn build_manifest(
    project: &str,
    entities: &BTreeMap<String, Entity>,
    graph: &Graph,
) -> Manifest {
    let entries = entities
        .iter()
        .map(|(id, e)| {
            let edges = graph.edges.get(id);
            let out: Vec<String> = edges
                .map(|x| x.outgoing.iter().cloned().collect())
                .unwrap_or_default();
            let inc: Vec<String> = edges
                .map(|x| x.incoming.iter().cloned().collect())
                .unwrap_or_default();
            ManifestEntry {
                id: id.clone(),
                title: e.name.clone(),
                kind: e.kind.label(),
                aliases: e.aliases.clone(),
                path: e.source_path.clone(),
                page: format!("{id}.md"),
                summary: e.summary.clone(),
                degree_in: inc.len(),
                degree_out: out.len(),
                pagerank: *graph.pagerank.get(id).unwrap_or(&0.0),
                token_estimate: token_estimate(&e.body),
                neighbors_out: out,
                neighbors_in: inc,
            }
        })
        .collect();
    Manifest {
        project: project.to_string(),
        entries,
    }
}

pub fn render_index_json(m: &Manifest) -> String {
    serde_json::to_string_pretty(m).expect("manifest serialization is infallible")
}

pub fn render_index_md(m: &Manifest) -> String {
    let mut out = format!("# {} — Index\n\n", m.project);
    out.push_str(&format!("{} pages.\n\n", m.entries.len()));
    for e in &m.entries {
        let summary = e.summary.as_deref().unwrap_or("");
        out.push_str(&format!(
            "- [{}]({}) — `{}` (in {}, out {}): {}\n",
            e.title, e.page, e.kind, e.degree_in, e.degree_out, summary
        ));
    }
    out
}

pub fn render_llms_txt(m: &Manifest) -> String {
    let mut out = format!("# {}\n\n", m.project);
    out.push_str(&format!(
        "> Generated wiki of {} pages. Start from this index; do not re-crawl the source.\n\n",
        m.entries.len()
    ));

    // Split by centrality: below-median pagerank goes under ## Optional.
    let mut ranks: Vec<f64> = m.entries.iter().map(|e| e.pagerank).collect();
    ranks.sort_by(|a, b| a.total_cmp(b));
    let median = ranks.get(ranks.len() / 2).copied().unwrap_or(0.0);

    out.push_str("## Docs\n\n");
    for e in m.entries.iter().filter(|e| e.pagerank >= median) {
        out.push_str(&format!(
            "- [{}]({}): {}\n",
            e.title,
            e.page,
            e.summary.as_deref().unwrap_or("")
        ));
    }
    out.push_str("\n## Optional\n\n");
    for e in m.entries.iter().filter(|e| e.pagerank < median) {
        out.push_str(&format!(
            "- [{}]({}): {}\n",
            e.title,
            e.page,
            e.summary.as_deref().unwrap_or("")
        ));
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
- Pages cross-link with `[[slug|Name]]` wikilinks (resolvable in Obsidian/Quartz and in `wiki lint`); each has Metadata / Related / Referenced By / Body sections.\n\
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
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::model::SourceKind;

    fn ent(id: &str, name: &str, body: &str, summary: Option<&str>) -> Entity {
        Entity {
            id: id.into(),
            name: name.into(),
            aliases: vec![],
            created: String::new(),
            body: body.into(),
            source_path: format!("{id}.txt"),
            kind: SourceKind::Text,
            content_hash: [0u8; 32],
            summary: summary.map(String::from),
            symbols: vec![],
            imports: vec![],
            defined: vec![],
        }
    }

    fn setup() -> (BTreeMap<String, Entity>, Graph) {
        let ents: BTreeMap<String, Entity> = vec![
            ent("alpha", "Alpha", "mentions Beta", Some("Alpha summary.")),
            ent("beta", "Beta", "nothing", None),
        ]
        .into_iter()
        .map(|e| (e.id.clone(), e))
        .collect();
        let g = build_graph(&ents);
        (ents, g)
    }

    #[test]
    fn manifest_has_sorted_entries_with_degrees() {
        let (ents, g) = setup();
        let m = build_manifest("proj", &ents, &g);
        assert_eq!(m.entries[0].id, "alpha");
        assert_eq!(m.entries[0].degree_out, 1);
        assert_eq!(m.entries[1].degree_in, 1); // beta referenced by alpha
    }

    #[test]
    fn index_json_is_valid_json_and_llms_txt_has_h1() {
        let (ents, g) = setup();
        let m = build_manifest("My Project", &ents, &g);
        let json = render_index_json(&m);
        let _v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(render_llms_txt(&m).starts_with("# My Project"));
        assert!(render_agents_md("My Project").contains("index.json"));
    }

    #[test]
    fn token_estimate_is_chars_over_four() {
        assert_eq!(token_estimate("abcdefgh"), 2);
    }

    #[test]
    fn agents_md_documents_slug_piped_link_format() {
        let md = render_agents_md("proj");
        assert!(
            md.contains("[[slug|Name]]"),
            "AGENTS.md should describe the slug-piped link format: {md}"
        );
        assert!(
            !md.contains("`[[Name]]`"),
            "AGENTS.md still describes the old [[Name]] format: {md}"
        );
    }
}
