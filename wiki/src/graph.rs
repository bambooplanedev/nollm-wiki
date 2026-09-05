//! Link graph: wikilink and import edges between pages, orphan detection, and `PageRank` centrality with damping.

use crate::formats::code::module_stem;
use crate::model::{Edges, Entity, Graph, SourceKind};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9']+").unwrap());
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
fn resolve_link(source_path: &str, target: &str) -> Option<String> {
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
    if entities.is_empty() {
        return Graph {
            edges,
            pagerank: BTreeMap::new(),
        };
    }
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
    let resolver = ImportResolver::new(entities);

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
        // page's module stem or one of its defined items (see `ImportResolver`).
        for imp in &ent.imports {
            targets.extend(resolver.resolve(imp).into_iter().filter(|tid| tid != eid));
        }
        for tid in targets {
            edges.get_mut(eid).unwrap().outgoing.insert(tid.clone());
            edges.get_mut(&tid).unwrap().incoming.insert(eid.clone());
        }
    }

    let pagerank = pagerank(&edges);
    Graph { edges, pagerank }
}

/// Import-string resolution, precomputed once per `build_graph`.
///
/// Segment matching, not module resolution: a `::` path is followed only
/// under a local root, and each remaining segment is looked up as a module
/// stem or a defined name. Two local crates that each own a `query.rs` both
/// resolve to the lexicographically smaller path.
struct ImportResolver<'a> {
    /// First segments a `::` path may start with and still be followed:
    /// `crate`, `super`, `self`, plus every local crate — the directory
    /// holding a `src/lib.rs` or `src/main.rs` page (`wiki` on this repo).
    local_roots: BTreeSet<&'a str>,
    /// Module stem → (`source_path`, page id); the smallest path wins.
    by_stem: BTreeMap<&'a str, (&'a str, &'a str)>,
    /// Defined name → page id when exactly one code page defines it,
    /// `None` once a second page does.
    by_defined: BTreeMap<&'a str, Option<&'a str>>,
}

impl<'a> ImportResolver<'a> {
    fn new(entities: &'a BTreeMap<String, Entity>) -> Self {
        let mut local_roots: BTreeSet<&str> = ["crate", "super", "self"].into_iter().collect();
        let mut by_stem: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
        let mut by_defined: BTreeMap<&str, Option<&str>> = BTreeMap::new();
        for (id, e) in entities {
            if !matches!(e.kind, SourceKind::Code { .. }) {
                continue;
            }
            let path = e.source_path.as_str();
            if let Some(root) = local_crate_root(path) {
                local_roots.insert(root);
            }
            let stem = module_stem(path);
            match by_stem.get(stem) {
                Some((kept, _)) if *kept <= path => {}
                _ => {
                    by_stem.insert(stem, (path, id.as_str()));
                }
            }
            for name in &e.defined {
                by_defined
                    .entry(name.as_str())
                    .and_modify(|owner| {
                        if *owner != Some(id.as_str()) {
                            *owner = None;
                        }
                    })
                    .or_insert(Some(id.as_str()));
            }
        }
        Self {
            local_roots,
            by_stem,
            by_defined,
        }
    }

    /// Every code page one import string refers to.
    fn resolve(&self, imp: &str) -> BTreeSet<String> {
        let mut segments = imp
            .split(|c: char| matches!(c, '.' | '/' | ':' | '{' | '}' | ',') || c.is_whitespace())
            .filter(|s| !s.is_empty());
        if imp.contains("::") {
            // The first segment of a Rust path is a crate root, never a
            // module of this tree, and only local roots are followed.
            match segments.next() {
                Some(root) if self.local_roots.contains(root) => {}
                _ => return BTreeSet::new(),
            }
        }
        segments
            .filter_map(|seg| self.resolve_segment(seg))
            .map(str::to_string)
            .collect()
    }

    fn resolve_segment(&self, seg: &str) -> Option<&'a str> {
        if let Some((_, id)) = self.by_stem.get(seg) {
            return Some(id);
        }
        self.by_defined.get(seg).copied().flatten()
    }
}

/// `wiki` for `wiki/src/lib.rs`; `None` for a crate at the corpus root (no
/// directory to name it) or for any file that is not a crate root.
fn local_crate_root(source_path: &str) -> Option<&str> {
    let mut parts = source_path.rsplit('/');
    let file = parts.next()?;
    if !matches!(file, "lib.rs" | "main.rs") || parts.next()? != "src" {
        return None;
    }
    parts.next()
}

pub fn orphan_ids(graph: &Graph) -> Vec<String> {
    graph
        .edges
        .iter()
        .filter(|(_, e)| e.incoming.is_empty())
        .map(|(id, _)| id.clone())
        .collect()
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
            defined: vec![],
            methods: vec![],
        }
    }

    fn map(v: Vec<Entity>) -> BTreeMap<String, Entity> {
        v.into_iter().map(|e| (e.id.clone(), e)).collect()
    }

    fn code(id: &str, name: &str, path: &str, body: &str) -> Entity {
        let mut e = ent(id, name, body);
        e.source_path = path.into();
        e.kind = SourceKind::Code {
            lang: "rust".into(),
        };
        e
    }

    #[test]
    fn refers_to_module_accepts_paths_declarations_and_filenames_only() {
        for yes in [
            "text::extract(x)",
            "use crate::text::Extract;",
            "see ::text for details",
            "mod text;",
            "pub mod text {",
            "pub(crate) mod   text;",
            "lives in src/text.rs",
            "text.rs",
        ] {
            assert!(refers_to_module(yes, "text", "rs"), "should match: {yes}");
        }
        for no in [
            "the text of the page",
            "body_text::x",
            "text_extractor::run()",
            "in context::here",
            "`text`",
            "SourceKind::Text",
            "ContentBlock::text(\"x\")",
            "text.rsx",
            "text.json",
            "method text;",
            "",
        ] {
            assert!(
                !refers_to_module(no, "text", "rs"),
                "should not match: {no}"
            );
        }
        // The extension is the target's own, not a fixed list.
        assert!(refers_to_module("from models.py", "models", "py"));
        assert!(!refers_to_module("from models.py", "models", "rs"));
    }

    #[test]
    fn a_prose_mention_of_a_code_page_is_not_an_edge_but_a_path_is() {
        let text = code("text", "Text", "src/text.rs", "");
        let notes = ent("notes", "Notes", "the text of the page is long");
        let mut user = code("user", "User", "src/user.rs", "use crate::text::Extract;");
        user.imports = vec![]; // pin the filter, not the resolver
        let g = build_graph(&map(vec![text, notes, user]));
        assert!(
            !g.edges["notes"].outgoing.contains("text"),
            "{:?}",
            g.edges["notes"]
        );
        assert!(
            g.edges["user"].outgoing.contains("text"),
            "{:?}",
            g.edges["user"]
        );
        assert!(g.edges["text"].incoming.contains("user"));
    }

    #[test]
    fn multi_word_code_titles_are_filtered_too() {
        let er = code(
            "extract_rust",
            "Extract Rust",
            "src/formats/extract_rust.rs",
            "",
        );
        let prose = ent("a", "Alpha", "Extract Rust handles pub gating");
        let file = ent("b", "Beta", "gating lives in extract_rust.rs today");
        let g = build_graph(&map(vec![er, prose, file]));
        assert!(!g.edges["a"].outgoing.contains("extract_rust"));
        assert!(g.edges["b"].outgoing.contains("extract_rust"));
    }

    #[test]
    fn text_and_markdown_targets_keep_prose_mentions() {
        let target = ent("text", "Text", "a note titled Text");
        let src = ent("a", "Alpha", "the text of this note");
        let g = build_graph(&map(vec![target, src]));
        assert!(g.edges["a"].outgoing.contains("text"));
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
    fn resolve_link_walks_relative_paths_and_skips_external_targets() {
        assert_eq!(
            resolve_link("scripts/README.md", "../README.md#self-hosting").as_deref(),
            Some("README.md")
        );
        assert_eq!(
            resolve_link("README.md", "wiki/docs/ARCHITECTURE.md").as_deref(),
            Some("wiki/docs/ARCHITECTURE.md")
        );
        assert_eq!(
            resolve_link("a/b/c.md", "./../d.md").as_deref(),
            Some("a/d.md")
        );
        assert_eq!(resolve_link("README.md", "../outside.md"), None);
        assert_eq!(resolve_link("README.md", "#anchor"), None);
        assert_eq!(resolve_link("README.md", "https://example.org/x.md"), None);
        assert_eq!(
            resolve_link("README.md", "mailto:someone@example.org"),
            None
        );
    }

    #[test]
    fn markdown_link_to_a_source_path_creates_an_edge_without_a_mention() {
        let mut a = ent(
            "a",
            "Alpha",
            "see [the other page](docs/b.md \"title\") and [x](nope.md)",
        );
        a.source_path = "README.md".into();
        let mut b = ent("b", "Beta", "nothing");
        b.source_path = "docs/b.md".into();
        let g = build_graph(&map(vec![a, b]));
        assert!(g.edges["a"].outgoing.contains("b"));
        assert!(g.edges["b"].incoming.contains("a"));
        assert!(g.edges["b"].outgoing.is_empty());
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

    #[test]
    fn local_crate_root_is_the_directory_holding_src_lib_or_main() {
        assert_eq!(local_crate_root("wiki/src/lib.rs"), Some("wiki"));
        assert_eq!(local_crate_root("tools/cli/src/main.rs"), Some("cli"));
        assert_eq!(local_crate_root("src/lib.rs"), None);
        assert_eq!(local_crate_root("wiki/src/graph.rs"), None);
        assert_eq!(local_crate_root("wiki/lib.rs"), None);
    }

    #[test]
    fn imports_resolve_every_segment_by_stem_and_defined_name() {
        let mut model = code("model", "Model", "wiki/src/model.rs", "");
        model.defined = vec!["Entity".into(), "Graph".into()];
        let formats = code("formats", "Formats", "wiki/src/formats/mod.rs", "");
        let codepage = code("code", "Code", "wiki/src/formats/code.rs", "");
        let mut summary = code("summary", "Summary", "wiki/src/formats/summary.rs", "");
        summary.defined = vec!["summarize".into()];
        let mut src_query = code("src_query", "Src Query", "wiki/src/query.rs", "");
        src_query.defined = vec!["Wiki".into(), "PackBudget".into()];
        let tests_query = code("tests_query", "Tests Query", "wiki/tests/query.rs", "");
        let mut lib = code("lib", "Lib", "wiki/src/lib.rs", "");
        lib.defined = vec!["compile".into(), "CompileOptions".into()];
        let readme = ent("wiki", "wiki", "the wiki README");
        let mut user = code("user", "User", "wiki/src/user.rs", "");
        user.imports = vec![
            "crate::model::{Entity, Graph}".into(),
            "crate::formats::code::CodeExtractor".into(),
            "crate::formats::{summarize, Extractor}".into(),
            "wiki::query::Wiki".into(),
            "wiki::{compile, CompileOptions}".into(),
            "rmcp::model::CallToolResult".into(),
            "std::collections::BTreeMap".into(),
            "serde::Serialize".into(),
        ];
        let g = build_graph(&map(vec![
            model,
            formats,
            codepage,
            summary,
            src_query,
            tests_query,
            lib,
            readme,
            user,
        ]));
        let out: Vec<&str> = g.edges["user"]
            .outgoing
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(
            out,
            ["code", "formats", "lib", "model", "src_query", "summary"],
            "src/ path wins the shared stem `query`; struct names resolve through `defined`; \
             rmcp/std/serde roots resolve nothing; the README is never an import target"
        );
    }

    #[test]
    fn dotted_and_relative_imports_resolve_every_segment() {
        let text = code("text", "Text", "pkg/text.py", "");
        let mut consumer = code("consumer", "Consumer", "pkg/consumer.py", "");
        consumer.imports = vec![".text".into(), "os.path".into()];
        let g = build_graph(&map(vec![text, consumer]));
        assert!(g.edges["consumer"].outgoing.contains("text"));
        assert_eq!(g.edges["consumer"].outgoing.len(), 1);
    }

    #[test]
    fn a_defined_name_shared_by_two_pages_resolves_nothing() {
        let mut a = code("a", "Alpha", "src/a.rs", "");
        a.defined = vec!["Thing".into()];
        let mut b = code("b", "Beta", "src/b.rs", "");
        b.defined = vec!["Thing".into()];
        let mut user = code("user", "User", "src/user.rs", "");
        user.imports = vec!["crate::Thing".into()];
        let g = build_graph(&map(vec![a, b, user]));
        assert!(g.edges["user"].outgoing.is_empty());
    }
}
