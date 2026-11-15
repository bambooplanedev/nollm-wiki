//! Link graph: mention edges (code-shaped when the target is a code page), import edges (also kept apart), markdown-link edges, orphan detection, and `PageRank` centrality with damping.

use crate::formats::code::module_stem;
use crate::model::{Edges, Entity, Graph, SourceKind};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;
use wiki_code::imports::Imports;

// Letters and combining marks in any script; digits and `'` stay ASCII on purpose.
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{M}0-9']+").unwrap());
/// The target of an inline markdown link, `[text](target)`, up to the closing
/// paren or a space (which starts an optional title).
static MD_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\]\(([^)\s]+)").unwrap());

fn tokens(text: &str) -> Vec<String> {
    WORD.find_iter(text)
        .map(|m| m.as_str().to_lowercase())
        .collect()
}

/// first-word -> [(word-tuple, `target_id`)], longest tuple first.
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
/// advances by one token. `build_phrase_index` (see its own doc above) hands
/// back each first-word bucket sorted longest tuple first, so "first match"
/// here means "longest match": a candidate is only skipped in favor of a
/// shorter one if the longer one doesn't actually match at this position.
/// That sort order is therefore load-bearing — reordering or dropping it
/// would silently change which phrase wins and, with it, the wiki's graph
/// edges. `slice::starts_with` subsumes the explicit end-of-slice bound the
/// original loop checked (`end <= n`): it is false whenever the phrase would
/// run past the end of `toks`, so no separate bound is needed.
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

/// The project-relative path a markdown link target points at, resolved
/// lexically against the linking file's directory. `None` for an external
/// link (any scheme), a bare `#anchor`, or a target that climbs above the
/// project root. `../README.md#x` from `scripts/README.md` is `README.md`.
pub(crate) fn resolve_link(source_path: &str, target: &str) -> Option<String> {
    let target = target.split('#').next().unwrap_or("");
    if target.is_empty() || target.split('/').next().unwrap_or("").contains(':') {
        return None;
    }
    let mut parts: Vec<&str> = source_path
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').collect())
        .unwrap_or_default();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// Ids of the pages whose source path a markdown link in `body` resolves to.
fn link_targets(body: &str, source_path: &str, by_path: &BTreeMap<&str, &str>) -> BTreeSet<String> {
    MD_LINK
        .captures_iter(body)
        .filter_map(|c| resolve_link(source_path, &c[1]))
        .filter_map(|p| by_path.get(p.as_str()).map(|id| (*id).to_string()))
        .collect()
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Does `body` refer to the module `stem`, compiled from `<stem>.<ext>`, in
/// code shape? One of: a `stem::` or `::stem` path (the latter not a method
/// call, `::stem(`), a `mod stem;` / `mod stem {` declaration, or the
/// filename `stem.ext`. Every occurrence must sit on identifier boundaries,
/// so `body_text` never matches `text`; matching is case-sensitive, so
/// `SourceKind::Text` does not either. Plain prose and a bare `` `stem` `` do
/// not count: the 2026-09-05 code-reference-edges design measured backticks
/// alone as the precision leak (22 kept edges, 16 noise).
fn refers_to_module(body: &str, stem: &str, ext: &str) -> bool {
    let bytes = body.as_bytes();
    body.match_indices(stem).any(|(start, _)| {
        let end = start + stem.len();
        let left_free = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after = bytes.get(end).copied();
        let right_free = after.is_none_or(|b| !is_ident_byte(b));
        let path_out = left_free && bytes[end..].starts_with(b"::");
        let path_in = right_free && bytes[..start].ends_with(b"::") && after != Some(b'(');
        let declaration = right_free && is_mod_declaration(body, start, end);
        let filename = left_free
            && after == Some(b'.')
            && body[end + 1..].starts_with(ext)
            && bytes
                .get(end + 1 + ext.len())
                .is_none_or(|b| !is_ident_byte(*b));
        path_out || path_in || declaration || filename
    })
}

/// `mod <stem>;` or `mod <stem> {` with the stem at `body[start..end]`; any
/// visibility before `mod` is fine, `method <stem>` is not.
fn is_mod_declaration(body: &str, start: usize, end: usize) -> bool {
    let head = body[..start].trim_end_matches(' ');
    if head.len() == start || !head.ends_with("mod") {
        return false;
    }
    if head[..head.len() - 3]
        .bytes()
        .next_back()
        .is_some_and(is_ident_byte)
    {
        return false;
    }
    let tail = body[end..].trim_start_matches(' ');
    tail.starts_with(';') || tail.starts_with('{')
}

pub fn build_graph(entities: &BTreeMap<String, Entity>) -> Graph {
    let mut edges: BTreeMap<String, Edges> = entities
        .keys()
        .map(|k| (k.clone(), Edges::default()))
        .collect();
    let index = build_phrase_index(entities);
    let by_path: BTreeMap<&str, &str> = entities
        .iter()
        .map(|(id, e)| (e.source_path.as_str(), id.as_str()))
        .collect();
    // Code pages by id: (module stem, extension), for the mention filter.
    let code_pages: BTreeMap<&str, (&str, &str)> = entities
        .iter()
        .filter(|(_, e)| matches!(e.kind, SourceKind::Code { .. }))
        .map(|(id, e)| {
            let ext = e.source_path.rsplit('.').next().unwrap_or("");
            (id.as_str(), (module_stem(&e.source_path), ext))
        })
        .collect();
    let resolver = Imports::new(entities);

    for (eid, ent) in entities {
        let toks = tokens(&ent.body);
        // A mention links a code page only when it is code-shaped: a one-word
        // title such as `Text` or `Hash` otherwise draws an edge from every
        // prose use of the word (see `refers_to_module`).
        let mut targets: BTreeSet<String> = phrase_targets(&toks, &index, eid)
            .into_iter()
            .filter(|tid| {
                code_pages
                    .get(tid.as_str())
                    .is_none_or(|(stem, ext)| refers_to_module(&ent.body, stem, ext))
            })
            .collect();
        // Markdown link edges: `[text](relative/path.md)` resolving to another
        // page's source path. A link is a deliberate reference, unlike a
        // mention, and is the only way a README reaches a page it never names.
        targets.extend(
            link_targets(&ent.body, &ent.source_path, &by_path)
                .into_iter()
                .filter(|tid| tid != eid),
        );
        // Import edges: every segment of an import string that names a code
        // page's module stem or one of its defined items (see `Imports`).
        let imports: BTreeSet<String> = ent
            .imports
            .iter()
            .flat_map(|imp| resolver.resolve(&ent.source_path, imp))
            .filter(|tid| tid != eid)
            .collect();
        for tid in &imports {
            edges.get_mut(eid).unwrap().imports.insert(tid.clone());
            edges.get_mut(tid).unwrap().imported_by.insert(eid.clone());
        }
        targets.extend(imports);
        for tid in targets {
            edges.get_mut(eid).unwrap().outgoing.insert(tid.clone());
            edges.get_mut(&tid).unwrap().incoming.insert(eid.clone());
        }
    }

    // Notes shape the edges but never the ranking: PageRank sees project pages only.
    let project: BTreeMap<String, Edges> = edges
        .iter()
        .filter(|(id, _)| !entities[*id].memory)
        .map(|(id, e)| {
            let outgoing = e.outgoing.iter().filter(|t| !entities[*t].memory);
            let kept = Edges {
                outgoing: outgoing.cloned().collect(),
                ..Edges::default()
            };
            (id.clone(), kept)
        })
        .collect();
    let pagerank = pagerank(&project);
    Graph { edges, pagerank }
}

/// Deterministic `PageRank`: fixed 40 iterations, damping 0.85, `BTree` node order.
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
mod tests;
