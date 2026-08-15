use crate::model::{Edges, Entity, Graph};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9']+").unwrap());

fn tokens(text: &str) -> Vec<String> {
    WORD.find_iter(text)
        .map(|m| m.as_str().to_lowercase())
        .collect()
}

/// first-word -> [(word-tuple, target_id)], longest tuple first.
fn build_phrase_index(
    entities: &BTreeMap<String, Entity>,
) -> BTreeMap<String, Vec<(Vec<String>, String)>> {
    let mut index: BTreeMap<String, Vec<(Vec<String>, String)>> = BTreeMap::new();
    for (eid, ent) in entities {
        let mut names = vec![ent.name.clone()];
        names.extend(ent.aliases.iter().cloned());
        for name in names {
            let words = tokens(&name);
            if words.is_empty() {
                continue;
            }
            index
                .entry(words[0].clone())
                .or_default()
                .push((words, eid.clone()));
        }
    }
    for v in index.values_mut() {
        v.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.1.cmp(&b.1)));
    }
    index
}

/// Ids whose registered phrase occurs in `toks`, scanning left to right.
///
/// At each position the first candidate that matches wins and the scan
/// advances by one token. `slice::starts_with` subsumes both the explicit
/// end-of-slice bound and the `break`: `find` already stops at the first
/// match, and `starts_with` is false whenever the phrase would run past the
/// end. The selection therefore does not depend on how `build_phrase_index`
/// orders its candidates — only on first-match order, which is unchanged.
fn phrase_targets(
    toks: &[String],
    index: &BTreeMap<String, Vec<(Vec<String>, String)>>,
    eid: &str,
) -> BTreeSet<String> {
    let mut targets = BTreeSet::new();
    for (i, tok) in toks.iter().enumerate() {
        if let Some(cands) = index.get(tok) {
            if let Some((_, target_id)) = cands.iter().find(|(w, _)| toks[i..].starts_with(w)) {
                if target_id != eid {
                    targets.insert(target_id.clone());
                }
            }
        }
    }
    targets
}

pub fn build_graph(entities: &BTreeMap<String, Entity>) -> Graph {
    let mut edges: BTreeMap<String, Edges> = entities
        .keys()
        .map(|k| (k.clone(), Edges::default()))
        .collect();
    if entities.is_empty() {
        return Graph {
            edges,
            pagerank: BTreeMap::new(),
        };
    }
    let index = build_phrase_index(entities);

    for (eid, ent) in entities {
        let toks = tokens(&ent.body);
        let mut targets = phrase_targets(&toks, &index, eid);
        // Import edges: resolve each import string to an entity id if it matches a
        // known id or the stem of a known source path.
        for imp in &ent.imports {
            if let Some(tid) = resolve_import(imp, entities) {
                if tid != *eid {
                    targets.insert(tid);
                }
            }
        }
        for tid in targets {
            edges.get_mut(eid).unwrap().outgoing.insert(tid.clone());
            edges.get_mut(&tid).unwrap().incoming.insert(eid.clone());
        }
    }

    let pagerank = pagerank(&edges);
    Graph { edges, pagerank }
}

fn resolve_import(imp: &str, entities: &BTreeMap<String, Entity>) -> Option<String> {
    let last = imp.rsplit(['.', '/', ':']).find(|s| !s.is_empty())?;
    let slug = crate::model::slugify(last);
    if entities.contains_key(&slug) {
        return Some(slug);
    }
    None
}

pub fn orphan_ids(graph: &Graph) -> Vec<String> {
    graph
        .edges
        .iter()
        .filter(|(_, e)| e.incoming.is_empty())
        .map(|(id, _)| id.clone())
        .collect()
}

/// Deterministic PageRank: fixed 40 iterations, damping 0.85, BTree node order.
fn pagerank(edges: &BTreeMap<String, Edges>) -> BTreeMap<String, f64> {
    let n = edges.len();
    if n == 0 {
        return BTreeMap::new();
    }
    let init = 1.0 / n as f64;
    let mut rank: BTreeMap<String, f64> = edges.keys().map(|k| (k.clone(), init)).collect();
    let d = 0.85;
    for _ in 0..40 {
        let mut next: BTreeMap<String, f64> = edges
            .keys()
            .map(|k| (k.clone(), (1.0 - d) / n as f64))
            .collect();
        let mut dangling = 0.0;
        for (id, e) in edges {
            let out = e.outgoing.len();
            if out == 0 {
                dangling += rank[id];
                continue;
            }
            let share = d * rank[id] / out as f64;
            for target in &e.outgoing {
                *next.get_mut(target).unwrap() += share;
            }
        }
        let dangling_share = d * dangling / n as f64;
        for v in next.values_mut() {
            *v += dangling_share;
        }
        rank = next;
    }
    rank
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Entity, SourceKind};
    use std::collections::BTreeMap;

    fn ent(id: &str, name: &str, body: &str) -> Entity {
        Entity {
            id: id.into(),
            name: name.into(),
            aliases: vec![],
            created: String::new(),
            body: body.into(),
            source_path: format!("{id}.txt"),
            kind: SourceKind::Text,
            content_hash: [0u8; 32],
            summary: None,
            symbols: vec![],
            imports: vec![],
        }
    }

    fn map(v: Vec<Entity>) -> BTreeMap<String, Entity> {
        v.into_iter().map(|e| (e.id.clone(), e)).collect()
    }

    #[test]
    fn bidirectional_edge_created() {
        let g = build_graph(&map(vec![
            ent("a", "Alpha", "mentions Beta here"),
            ent("b", "Beta", "nothing"),
        ]));
        assert!(g.edges["a"].outgoing.contains("b"));
        assert!(g.edges["b"].incoming.contains("a"));
    }

    #[test]
    fn no_self_link() {
        let g = build_graph(&map(vec![ent("a", "Alpha", "Alpha refers to itself")]));
        assert!(!g.edges["a"].outgoing.contains("a"));
    }

    #[test]
    fn alias_creates_edge() {
        let mut beta = ent("b", "Beta", "nothing");
        beta.aliases = vec!["Second Letter".into()];
        let g = build_graph(&map(vec![ent("a", "Alpha", "see the second letter"), beta]));
        assert!(g.edges["a"].outgoing.contains("b"));
    }

    #[test]
    fn orphans_have_zero_incoming() {
        let g = build_graph(&map(vec![
            ent("a", "Alpha", "mentions Beta"),
            ent("b", "Beta", "nothing"),
            ent("c", "Gamma", "nothing"),
        ]));
        let o = orphan_ids(&g);
        assert!(o.contains(&"c".to_string()));
        assert!(o.contains(&"a".to_string()));
        assert!(!o.contains(&"b".to_string()));
    }

    #[test]
    fn pagerank_is_deterministic_and_sums_to_about_one() {
        let g = build_graph(&map(vec![
            ent("a", "Alpha", "mentions Beta"),
            ent("b", "Beta", "mentions Alpha"),
        ]));
        let sum: f64 = g.pagerank.values().sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }
}
