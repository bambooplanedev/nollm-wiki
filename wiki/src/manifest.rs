//! Manifest and index rendering: the page index (JSON and markdown), the llms-txt and AGENTS files, the graph export, and the per-page token estimate.

use crate::model::{CodeItem, Entity, Graph};
use serde::Serialize;
use std::collections::BTreeMap;

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
            let out: Vec<String> = edges
                .map(|x| x.outgoing.iter().cloned().collect())
                .unwrap_or_default();
            let inc: Vec<String> = edges
                .map(|x| x.incoming.iter().cloned().collect())
                .unwrap_or_default();
            let imports_out: Vec<String> = edges
                .map(|x| x.imports.iter().cloned().collect())
                .unwrap_or_default();
            let imports_in: Vec<String> = edges
                .map(|x| x.imported_by.iter().cloned().collect())
                .unwrap_or_default();
            ManifestEntry {
                id: id.clone(),
                title: e.name.clone(),
                kind: e.kind.label(),
                aliases: e.aliases.clone(),
                tags: e.tags.clone(),
                path: e.source_path.clone(),
                page: format!("{id}.md"),
                summary: e.summary.clone(),
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
/// catalog several times over and `index.json` is served whole as
/// `wiki://index`. Compact, as only tools read it.
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
    for e in &m.entries {
        out.push_str(&format!(
            "- [{}]({}) — `{}` (in {}, out {}){}\n",
            e.title,
            e.page,
            e.kind,
            e.degree_in,
            e.degree_out,
            summary_suffix(e)
        ));
    }
    out
}

/// `: <summary>` when the entry has one, nothing when it does not — so a
/// page without a summary never ends its index line in a dangling colon.
fn summary_suffix(e: &ManifestEntry) -> String {
    e.summary
        .as_deref()
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
    let mut ranks: Vec<f64> = m.entries.iter().map(|e| e.pagerank).collect();
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
- A module's map: `wiki page <id>` (one line per symbol, `start-end kind signature`); `--full` for the whole page.\n\
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
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::model::{CodeItemKind, SourceKind};

    fn ent(id: &str, name: &str, body: &str, summary: Option<&str>) -> Entity {
        Entity {
            id: id.into(),
            name: name.into(),
            aliases: vec![],
            tags: vec![],
            created: String::new(),
            body: body.into(),
            source_path: format!("{id}.txt"),
            kind: SourceKind::Text,
            content_hash: [0u8; 32],
            summary: summary.map(String::from),
            symbols: vec![],
            imports: vec![],
            defined: vec![],
            methods: vec![],
            items: vec![],
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
        let m = build_manifest("proj", "", &ents, &g);
        assert_eq!(m.entries[0].id, "alpha");
        assert_eq!(m.entries[0].degree_out, 1);
        assert_eq!(m.entries[1].degree_in, 1); // beta referenced by alpha
    }

    #[test]
    fn index_json_is_valid_json_and_llms_txt_has_h1() {
        let (ents, g) = setup();
        let m = build_manifest("My Project", "", &ents, &g);
        let json = render_index_json(&m);
        let _v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(render_llms_txt(&m).starts_with("# My Project"));
        let agents = render_agents_md("My Project");
        assert!(agents.contains("index.json"));
        assert!(agents.contains("items.json") && agents.contains("wiki page"));
    }

    #[test]
    fn a_page_without_a_summary_ends_its_index_lines_cleanly() {
        let (ents, g) = setup();
        let m = build_manifest("proj", "", &ents, &g);
        let llms = render_llms_txt(&m);
        let md = render_index_md(&m);
        assert!(
            llms.contains("- [Alpha](alpha.md): Alpha summary.\n"),
            "{llms}"
        );
        assert!(llms.contains("- [Beta](beta.md)\n"), "{llms}");
        assert!(md.contains("(in 1, out 0)\n"), "{md}");
        assert!(!llms.contains(": \n") && !md.contains(": \n"));
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

    #[test]
    fn index_json_carries_defined_and_methods_as_the_last_entry_keys() {
        let (mut ents, g) = setup();
        let alpha_e = ents.get_mut("alpha").unwrap();
        alpha_e.defined = vec!["Alpha".into(), "build_alpha".into()];
        alpha_e.methods = vec!["build".into()];
        let m = build_manifest("My Project", "", &ents, &g);
        let json = render_index_json(&m);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let alpha = &v["entries"][0];
        assert_eq!(alpha["id"], "alpha");
        assert_eq!(
            alpha["defined"],
            serde_json::json!(["Alpha", "build_alpha"])
        );
        assert_eq!(alpha["methods"], serde_json::json!(["build"]));
        assert_eq!(v["entries"][1]["defined"], serde_json::json!([]));
        assert_eq!(v["entries"][1]["methods"], serde_json::json!([]));
        // Key order in the pretty output: neighbors_in < defined < methods,
        // so `methods` is the last key of every entry.
        let a_text = &json[json.find("\"id\": \"alpha\"").unwrap()..];
        let pos_aliases = a_text.find("\"aliases\"").unwrap();
        let pos_tags = a_text.find("\"tags\"").unwrap();
        let pos_path = a_text.find("\"path\"").unwrap();
        assert!(pos_aliases < pos_tags && pos_tags < pos_path, "{json}");
        let pos_neighbors_in = a_text.find("\"neighbors_in\"").unwrap();
        let pos_defined = a_text.find("\"defined\"").unwrap();
        let pos_methods = a_text.find("\"methods\"").unwrap();
        assert!(
            pos_neighbors_in < pos_defined && pos_defined < pos_methods,
            "expected neighbors_in < defined < methods:\n{json}"
        );
    }

    #[test]
    fn items_json_keys_each_code_page_by_id_and_skips_pages_without_items() {
        let (mut ents, _) = setup();
        ents.get_mut("beta").unwrap().items = vec![
            CodeItem {
                kind: CodeItemKind::Fn,
                name: "build_beta".into(),
                owner: None,
                signature: "pub fn build_beta()".into(),
                start_line: 1,
                end_line: 1,
                doc: None,
                doc_text: None,
                public: true,
                twin: None,
                test: false,
            },
            CodeItem {
                kind: CodeItemKind::Method,
                name: "build".into(),
                owner: Some("Beta".into()),
                signature: "fn build()".into(),
                start_line: 3,
                end_line: 5,
                doc: Some("Builds.".into()),
                doc_text: None,
                public: false,
                twin: Some("beta.cpp:2-4".into()),
                test: true,
            },
        ];
        // Each item's keys in declaration order, the `None` ones skipped;
        // `alpha` has no items, so it has no key.
        assert_eq!(
            render_items_json(&ents),
            concat!(
                r#"{"beta":["#,
                r#"{"kind":"fn","name":"build_beta","signature":"pub fn build_beta()","start_line":1,"end_line":1,"public":true},"#,
                r#"{"kind":"method","name":"build","owner":"Beta","signature":"fn build()","start_line":3,"end_line":5,"doc":"Builds.","public":false,"twin":"beta.cpp:2-4","test":true}]}"#
            )
        );
    }
}
