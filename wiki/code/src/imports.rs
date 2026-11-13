//! Import-string resolution: the code units an import names.

use crate::support::{is_shell, module_stem, source_rank};
use crate::CodeUnit;
use std::collections::{BTreeMap, BTreeSet};

/// Import-string resolution, precomputed once per map of units.
///
/// Segment matching, not module resolution: a `::` path is followed only
/// under a local root, and each remaining segment is looked up as a module
/// stem or a defined name. Two local crates that each own a `query.rs` both
/// resolve to the lower `source_rank`, then the lexicographically smaller
/// path (see `by_stem`'s own doc for why rank comes first).
///
/// A C/C++ `#include`, by contrast, names a path, not a sequence of module
/// segments, and is resolved separately by `resolve_include`.
///
/// Ties between two files are broken by the smallest key. A caller keyed by
/// path and one keyed by some other name can therefore disagree when two
/// headers share an include suffix.
pub struct Imports<'a> {
    /// First segments a `::` path may start with and still be followed:
    /// `crate`, `super`, `self`, plus every local crate — the directory
    /// holding a `src/lib.rs` or `src/main.rs` unit (`wiki` on this repo).
    local_roots: BTreeSet<&'a str>,
    /// Module stem → (`source_rank`, the unit's path, the map's key).
    ///
    /// The two-pages-per-component design gives a `.h`/`.cpp` pair the same
    /// stem, and the header — not whichever path sorts first — is what an
    /// `#include` of the bare stem should mean, so the tie-break is
    /// `source_rank` first (a header always ranks below its implementation)
    /// and the path only to break a tie between two files of the same
    /// rank. Without a header extension every file ties on rank and this
    /// reduces to the smallest-path rule.
    by_stem: BTreeMap<&'a str, (u8, &'a str, &'a str)>,
    /// Defined name → (`source_rank`, the map's key) of the lowest-rank claimant,
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
    /// The unit's path → the map's key, for `resolve_include`'s suffix match.
    by_path: BTreeMap<&'a str, &'a str>,
}

impl<'a> Imports<'a> {
    /// The lookup tables for `units`, keyed as the map is.
    pub fn new<U: CodeUnit>(units: &'a BTreeMap<String, U>) -> Self {
        let mut local_roots: BTreeSet<&str> = ["crate", "super", "self"].into_iter().collect();
        let mut by_stem: BTreeMap<&str, (u8, &str, &str)> = BTreeMap::new();
        let mut by_defined: BTreeMap<&str, Option<(u8, &str)>> = BTreeMap::new();
        let mut by_path: BTreeMap<&str, &str> = BTreeMap::new();
        for (id, e) in units {
            if e.lang().is_none() {
                continue;
            }
            let path = e.path();
            by_path.insert(path, id.as_str());
            if is_shell(path) {
                continue;
            }
            if let Some(root) = local_crate_root(path) {
                local_roots.insert(root);
            }
            let stem = module_stem(path);
            let rank = source_rank(path);
            match by_stem.get(stem) {
                // A header declares the component and is what `#include`
                // names, so it owns the stem even though `.cpp` sorts first.
                Some((kept_rank, kept_path, _)) if (*kept_rank, *kept_path) <= (rank, path) => {}
                _ => {
                    by_stem.insert(stem, (rank, path, id.as_str()));
                }
            }
            for name in e.defined() {
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

    /// The keys one import string in the file at `from_path` names: a shell
    /// file's `source` argument by path (`resolve_source`), any other import
    /// by `resolve_import`.
    pub fn resolve(&self, from_path: &str, import: &str) -> BTreeSet<String> {
        if is_shell(from_path) {
            self.resolve_source(import, from_path)
        } else {
            self.resolve_import(import)
        }
    }

    /// The keys of every code unit one import string refers to.
    ///
    /// A C/C++ include is routed to `resolve_include` instead of segment
    /// resolution when it is angle-bracketed (`<vector>`) or its path ends in
    /// a C/C++ header extension (`.h`, `.hpp`, `.hh`, `.hxx`) — see
    /// `is_cpp_include`'s own doc for why this predicate, and not the shape
    /// test "has both `/` and `.`", is the one that must gate this. Every
    /// other import (Rust, Python, JS/TS, Go) keeps the segment path by
    /// construction.
    fn resolve_import(&self, imp: &str) -> BTreeSet<String> {
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
    /// the bracket-stripped path as a suffix of a unit's path yields
    /// at most one edge and resolves a project's own `<mylib/core.h>` while
    /// leaving third-party headers unresolved, since no unit's path ends in
    /// `boost/algorithm/string.hpp`.
    ///
    /// The match must land on a path-segment boundary: `src.ends_with(path)`
    /// alone would let `other/core/book/orderbook.h` match an include of
    /// `book/orderbook.h`, so the byte immediately before the suffix must be
    /// `/` (or the suffix must be the whole path).
    ///
    /// More than one unit's path can end in the same suffix (two `-I` roots
    /// shadowing a name); determinism requires picking one candidate the
    /// same way on every run, so the candidate keys are collected, sorted,
    /// and only the lexicographically smallest is kept.
    fn resolve_include(&self, imp: &str) -> BTreeSet<String> {
        let path = imp
            .trim()
            .trim_matches(|c| c == '<' || c == '>' || c == '"');
        if path.is_empty() {
            return BTreeSet::new();
        }
        let mut hits = self.suffix_hits(path);
        hits.sort_unstable();
        hits.truncate(1);
        hits.into_iter().map(str::to_string).collect()
    }

    /// Keys of the units whose path ends in `path` on a `/` boundary.
    fn suffix_hits(&self, path: &str) -> Vec<&'a str> {
        self.by_path
            .iter()
            .filter(|(src, _)| {
                src.ends_with(path)
                    && (src.len() == path.len()
                        || src.as_bytes()[src.len() - path.len() - 1] == b'/')
            })
            .map(|(_, id)| *id)
            .collect()
    }

    /// The key of the unit a shell `source` / `.` argument names, by path only.
    ///
    /// Quotes are dropped. A literal path is tried as written, relative to
    /// the importing file's directory and then the project root. Otherwise
    /// (a variable in the way) only the tail after the last expansion is
    /// used. A suffix match counts only when exactly one unit matches; an
    /// empty tail (`source "$1"`) gives no edge.
    fn resolve_source(&self, imp: &str, from: &str) -> BTreeSet<String> {
        let arg: String = imp.chars().filter(|c| !matches!(c, '"' | '\'')).collect();
        let literal = !arg.contains('$');
        let tail = match arg.rfind('$') {
            None => &arg[..],
            Some(d) => {
                let name = arg[d + 1..]
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map_or(arg.len(), |i| d + 1 + i);
                let close = arg.rfind(['}', ')']).map_or(0, |i| i + 1);
                &arg[name.max(close)..]
            }
        };
        let tail = tail.trim_start_matches("./").trim_start_matches('/');
        if tail.is_empty() {
            return BTreeSet::new();
        }
        if literal {
            let dir = from.rsplit_once('/').map_or("", |(d, _)| d);
            for base in ["", dir] {
                if let Some(id) = self.by_path.get(normalize(base, tail).as_str()) {
                    return BTreeSet::from([(*id).to_string()]);
                }
            }
        }
        let tail = tail.trim_start_matches("../");
        match self.suffix_hits(tail)[..] {
            [id] => BTreeSet::from([id.to_string()]),
            _ => BTreeSet::new(),
        }
    }
}

/// `base/rel` with `.` and `..` components folded away.
fn normalize(base: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in base.split('/').chain(rel.split('/')) {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    parts.join("/")
}

/// Does an import string name a C/C++ include, as opposed to a module path?
///
/// The obvious shape test — angle-bracketed, or containing both `/` and `.`
/// — is wrong: a TypeScript `import x from "./client"` contains both a `/`
/// and a `.` (in the leading `./`) and would be routed here, find no
/// path suffix match (no unit's path ends in `./client`), and
/// silently drop the import edge. Requiring a known C/C++ header extension
/// instead of a bare "has a dot" test keeps every other language's imports —
/// none of which end in `.h`/`.hpp`/`.hh`/`.hxx` — on the segment path.
///
/// A bracketed `<vector>` has no extension at all, so it is still routed on
/// the angle-bracket shape alone; `resolve_include` correctly resolves it to
/// nothing when no unit's path matches.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_crate_root_is_the_directory_holding_src_lib_or_main() {
        assert_eq!(local_crate_root("wiki/src/lib.rs"), Some("wiki"));
        assert_eq!(local_crate_root("tools/cli/src/main.rs"), Some("cli"));
        assert_eq!(local_crate_root("src/lib.rs"), None);
        assert_eq!(local_crate_root("wiki/src/graph.rs"), None);
        assert_eq!(local_crate_root("wiki/lib.rs"), None);
    }
}
