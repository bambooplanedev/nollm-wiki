//! Link graph: mention edges (code-shaped when the target is a code page), import edges (also kept apart), markdown-link edges, orphan detection, and `PageRank` centrality with damping.

use crate::formats::code::module_stem;
use crate::model::{Edges, Entity, Graph, SourceKind};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

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
        let imports: BTreeSet<String> = ent
            .imports
            .iter()
            .flat_map(|imp| resolver.resolve(imp))
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

    let pagerank = pagerank(&edges);
    Graph { edges, pagerank }
}

/// Import-string resolution, precomputed once per `build_graph`.
///
/// Segment matching, not module resolution: a `::` path is followed only
/// under a local root, and each remaining segment is looked up as a module
/// stem or a defined name. Two local crates that each own a `query.rs` both
/// resolve to the lower `source_rank`, then the lexicographically smaller
/// path (see `by_stem`'s own doc for why rank comes first).
///
/// A C/C++ `#include`, by contrast, names a path, not a sequence of module
/// segments, and is resolved separately by `resolve_include`.
struct ImportResolver<'a> {
    /// First segments a `::` path may start with and still be followed:
    /// `crate`, `super`, `self`, plus every local crate — the directory
    /// holding a `src/lib.rs` or `src/main.rs` page (`wiki` on this repo).
    local_roots: BTreeSet<&'a str>,
    /// Module stem → (`source_rank`, `source_path`, page id).
    ///
    /// The two-pages-per-component design gives a `.h`/`.cpp` pair the same
    /// stem, and the header — not whichever path sorts first — is what an
    /// `#include` of the bare stem should mean, so the tie-break is
    /// `source_rank` first (a header always ranks below its implementation)
    /// and `source_path` only to break a tie between two files of the same
    /// rank. Without a header extension every file ties on rank and this
    /// reduces to the smallest-path rule.
    by_stem: BTreeMap<&'a str, (u8, &'a str, &'a str)>,
    /// Defined name → (`source_rank`, page id) of the lowest-rank claimant,
    /// or `None` once two pages of the *same* rank both claim it.
    ///
    /// A C++ free function is declared in a header and defined in its
    /// `.cpp`, so both pages `defined` the same name by design — without
    /// preferring the lower rank, that collision would set every free
    /// function's entry to `None` and the by-name resolution path would go
    /// dead across the whole of C++. A higher-rank claimant never displaces
    /// a lower one; only a tie (both pages the same rank, as every
    /// pre-existing corpus is) still sets `None`.
    by_defined: BTreeMap<&'a str, Option<(u8, &'a str)>>,
    /// `source_path` → page id, for `resolve_include`'s suffix match.
    by_path: BTreeMap<&'a str, &'a str>,
}

impl<'a> ImportResolver<'a> {
    fn new(entities: &'a BTreeMap<String, Entity>) -> Self {
        let mut local_roots: BTreeSet<&str> = ["crate", "super", "self"].into_iter().collect();
        let mut by_stem: BTreeMap<&str, (u8, &str, &str)> = BTreeMap::new();
        let mut by_defined: BTreeMap<&str, Option<(u8, &str)>> = BTreeMap::new();
        let mut by_path: BTreeMap<&str, &str> = BTreeMap::new();
        for (id, e) in entities {
            if !matches!(e.kind, SourceKind::Code { .. }) {
                continue;
            }
            let path = e.source_path.as_str();
            by_path.insert(path, id.as_str());
            if crate::formats::code::is_shell(path) {
                continue;
            }
            if let Some(root) = local_crate_root(path) {
                local_roots.insert(root);
            }
            let stem = module_stem(path);
            let rank = crate::formats::code::source_rank(path);
            match by_stem.get(stem) {
                // A header declares the component and is what `#include`
                // names, so it owns the stem even though `.cpp` sorts first.
                Some((kept_rank, kept_path, _)) if (*kept_rank, *kept_path) <= (rank, path) => {}
                _ => {
                    by_stem.insert(stem, (rank, path, id.as_str()));
                }
            }
            for name in &e.defined {
                match by_defined.get(name.as_str()) {
                    // A header/implementation pair defines the same free
                    // function twice by design. Without this the by-name path
                    // would go dead for every free function in a C++ corpus.
                    Some(Some((kept_rank, kept_id))) if *kept_id != id.as_str() => {
                        if rank < *kept_rank {
                            by_defined.insert(name.as_str(), Some((rank, id.as_str())));
                        } else if rank == *kept_rank {
                            by_defined.insert(name.as_str(), None);
                        }
                    }
                    Some(_) => {}
                    None => {
                        by_defined.insert(name.as_str(), Some((rank, id.as_str())));
                    }
                }
            }
        }
        Self {
            local_roots,
            by_stem,
            by_defined,
            by_path,
        }
    }

    /// Every code page one import string refers to.
    ///
    /// A C/C++ include is routed to `resolve_include` instead of segment
    /// resolution when it is angle-bracketed (`<vector>`) or its path ends in
    /// a C/C++ header extension (`.h`, `.hpp`, `.hh`, `.hxx`) — see
    /// `is_cpp_include`'s own doc for why this predicate, and not the shape
    /// test "has both `/` and `.`", is the one that must gate this. Every
    /// other import (Rust, Python, JS/TS, Go) keeps the segment path by
    /// construction.
    fn resolve(&self, imp: &str) -> BTreeSet<String> {
        let trimmed = imp.trim();
        if is_cpp_include(trimmed) {
            return self.resolve_include(trimmed);
        }
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
        if let Some((_, _, id)) = self.by_stem.get(seg) {
            return Some(id);
        }
        self.by_defined
            .get(seg)
            .copied()
            .flatten()
            .map(|(_, id)| id)
    }

    /// An `#include` is a path, not a namespace path: resolve it as one.
    ///
    /// Segment-wise resolution drew an edge for every component, so
    /// `#include "core/book/orderbook.h"` hit pages named `core`, `book` and
    /// `h` as well as the intended one, and `<boost/algorithm/string.hpp>`
    /// invented edges to a project's own `algorithm` and `string`. Matching
    /// the bracket-stripped path as a suffix of a page's `source_path` yields
    /// at most one edge and resolves a project's own `<mylib/core.h>` while
    /// leaving third-party headers unresolved, since no page's path ends in
    /// `boost/algorithm/string.hpp`.
    ///
    /// The match must land on a path-segment boundary: `src.ends_with(path)`
    /// alone would let `other/core/book/orderbook.h` match an include of
    /// `book/orderbook.h`, so the byte immediately before the suffix must be
    /// `/` (or the suffix must be the whole path).
    ///
    /// More than one page's path can end in the same suffix (two `-I` roots
    /// shadowing a name); determinism requires picking one candidate the
    /// same way on every run, so the candidate ids are collected, sorted,
    /// and only the lexicographically smallest is kept.
    fn resolve_include(&self, imp: &str) -> BTreeSet<String> {
        let path = imp
            .trim()
            .trim_matches(|c| c == '<' || c == '>' || c == '"');
        if path.is_empty() {
            return BTreeSet::new();
        }
        let mut hits: Vec<&str> = self
            .by_path
            .iter()
            .filter(|(src, _)| {
                src.ends_with(path)
                    && (src.len() == path.len()
                        || src.as_bytes()[src.len() - path.len() - 1] == b'/')
            })
            .map(|(_, id)| *id)
            .collect();
        hits.sort_unstable();
        hits.truncate(1);
        hits.into_iter().map(str::to_string).collect()
    }
}

/// Does an import string name a C/C++ include, as opposed to a module path?
///
/// The obvious shape test — angle-bracketed, or containing both `/` and `.`
/// — is wrong: a TypeScript `import x from "./client"` contains both a `/`
/// and a `.` (in the leading `./`) and would be routed here, find no
/// `source_path` suffix match (no page's path ends in `./client`), and
/// silently drop the import edge. Requiring a known C/C++ header extension
/// instead of a bare "has a dot" test keeps every other language's imports —
/// none of which end in `.h`/`.hpp`/`.hh`/`.hxx` — on the segment path.
///
/// A bracketed `<vector>` has no extension at all, so it is still routed on
/// the angle-bracket shape alone; `resolve_include` correctly resolves it to
/// nothing when no page's path matches.
fn is_cpp_include(trimmed: &str) -> bool {
    if trimmed.starts_with('<') {
        return true;
    }
    let inner = trimmed.trim_matches('"');
    matches!(inner.rsplit('.').next(), Some("h" | "hpp" | "hh" | "hxx"))
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
            body: body.into(),
            source_path: format!("{id}.txt"),
            ..Entity::default()
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
    fn cyrillic_alias_creates_edge() {
        let mut ai = ent("ai", "Artificial intelligence", "About learning.");
        ai.aliases = vec!["ШІ".into()];
        let notes = ent("notes", "Notes", "Today we talked about ШІ.");
        let g = build_graph(&map(vec![ai, notes]));
        assert!(g.edges["notes"].outgoing.contains("ai"));
    }

    #[test]
    fn accented_word_is_not_split_into_a_false_mention() {
        let ve = ent("ve", "Ve", "Short.");
        let trip = ent("trip", "Trip", "We saw a naïve painting.");
        let g = build_graph(&map(vec![ve, trip]));
        assert!(!g.edges["trip"].outgoing.contains("ve"));
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
    fn import_edges_are_kept_apart_from_mentions() {
        let text = code("text", "Text", "pkg/text.py", "");
        let mut consumer = code("consumer", "Consumer", "pkg/consumer.py", "");
        consumer.imports = vec![".text".into()];
        let notes = ent("notes", "Notes", "see `text.py`");
        let g = build_graph(&map(vec![text, consumer, notes]));
        assert_eq!(g.edges["consumer"].imports, g.edges["consumer"].outgoing);
        assert!(g.edges["notes"].outgoing.contains("text"));
        assert!(g.edges["notes"].imports.is_empty());
        let t = &g.edges["text"];
        assert_eq!(t.incoming.len(), 2);
        assert_eq!(t.imported_by.iter().collect::<Vec<_>>(), ["consumer"]);
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

    #[test]
    fn a_ts_index_import_reaches_the_module_not_a_test_of_the_same_name() {
        // `backends` is the directory module; a test file named after it must
        // not take the stem and collect the importer's edge.
        let module = code("src_backends", "Backends", "src/backends/index.ts", "");
        let test = code(
            "tests_backends",
            "Tests Backends",
            "tests/backends.test.ts",
            "",
        );
        let mut app = code("app", "App", "src/app.ts", "");
        app.imports = vec!["./backends/index.js".into()];
        let g = build_graph(&map(vec![module, test, app]));
        let out: Vec<&str> = g.edges["app"].outgoing.iter().map(String::as_str).collect();
        assert_eq!(out, ["src_backends"]);
    }
}
