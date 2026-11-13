//! Call edges between code items: matched by name at compile time
//! (`resolve_calls`), followed at query time (`Wiki::calls`).
//!
//! Matching is by name alone, with no type inference, so the edges are the
//! same on every run. A name with several matches keeps them all: a method
//! called on a value of unknown type links every same-named method the
//! caller's file can see.

use crate::manifest::{largest_fit, token_estimate};
use crate::model::{CallSite, CallVia, CodeItem, CodeItemKind, Entity, Graph};
use crate::query::Wiki;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use wiki_code::index::{owner_key, sees, FnRef, Index};

/// The functions one call site names, from a function owned by `own`
/// on page `page`, which sees the pages `sees`; see `resolve_calls`.
fn resolve(
    index: &Index<Entity>,
    site: &CallSite,
    own: Option<&str>,
    page: usize,
    sees: &BTreeSet<usize>,
) -> Vec<FnRef> {
    let scope = index.scope(page, own, sees);
    let only = |v: Vec<FnRef>| if v.len() == 1 { v } else { Vec::new() };
    let own_methods = || scope.own_methods(site);
    let fn_here = || scope.fns_here(site);
    let fn_seen = || scope.fns_seen(site);
    let method_seen = || scope.methods_seen(site);
    let fn_only = || only(scope.fns_anywhere(site));
    let method_only = || only(scope.methods_anywhere(site));
    let qualified = |q: &str| scope.qualified(site, q);
    let first = |tries: &[&dyn Fn() -> Vec<FnRef>]| {
        tries
            .iter()
            .map(|t| t())
            .find(|v| !v.is_empty())
            .unwrap_or_default()
    };
    match &site.via {
        // A method of a base class is called bare too (C++).
        CallVia::Bare => first(&[&own_methods, &fn_here, &fn_seen, &method_seen, &fn_only]),
        CallVia::Path(q) if q == "Self" => own_methods(),
        CallVia::Path(q) if matches!(q.as_str(), "self" | "crate" | "super") => {
            first(&[&fn_here, &fn_seen, &fn_only])
        }
        CallVia::Path(q) => qualified(q.as_str()),
        CallVia::SelfRecv => first(&[&own_methods, &method_seen, &method_only]),
        CallVia::Member(Some(x)) if index.is_type(x.as_str()) || index.is_module(x.as_str()) => {
            let q = || qualified(x.as_str());
            first(&[&q, &method_seen, &method_only])
        }
        CallVia::Member(_) => first(&[&method_seen, &method_only]),
    }
}

/// Fills every function item's `calls` from its `call_sites`, which it
/// empties. A call site names its target one of four ways (see `CallVia`),
/// and the first rule that finds anything wins:
///
/// - `f()`: a method of the caller's own owner; a free function in the
///   caller's file, then in a file it sees; a method of a file it sees (a
///   C++ base class's); the one free function of that name in the project.
/// - `Q::f()`: `Self::f` is the caller's owner's; `self`/`crate`/`super`
///   read as a free `f()`; any other `Q` is a type (an owner whose last
///   segment is `Q`), a module (a file whose stem or directory is `Q`), or
///   a C++ namespace (a free function's innermost one is `Q`, a
///   `namespace Q = a::b;` alias read as `b`). A `Q` that is none of them
///   is outside the project: `std::f()` never reaches a project `f`.
/// - `self.f()`: the caller's owner's; a method of a file it sees; the one
///   method of that name.
/// - `x.f()`: as `x::f()` when `x` names a project type or module; a method
///   of a file the caller sees; the one method of that name.
///
/// Before any rule, a function whose parameters cannot take the call's
/// argument count (`Arity::fits`) is no candidate.
///
/// Only functions in the caller's language family (`Family::of`) count. The
/// files a caller sees are its own, the pages it imports, and theirs:
/// a `.cpp` reaches its types through its own header's includes. A header
/// declaration stands for its definition, which holds the body. A function
/// never calls itself here.
pub fn resolve_calls(entities: &mut BTreeMap<String, Entity>, graph: &Graph) {
    let index = Index::new(entities);
    let ids: Vec<&str> = entities.keys().map(String::as_str).collect();
    let page_of: BTreeMap<&str, usize> = ids.iter().enumerate().map(|(p, id)| (*id, p)).collect();
    let imports = |p: usize| {
        graph.edges.get(ids[p]).into_iter().flat_map(|e| {
            e.imports
                .iter()
                .filter_map(|t| page_of.get(t.as_str()).copied())
        })
    };
    let mut resolved: Vec<(usize, usize, BTreeMap<String, BTreeSet<usize>>)> = Vec::new();
    for (p, e) in entities.values().enumerate() {
        if e.items.iter().all(|it| it.call_sites.is_empty()) {
            continue;
        }
        let sees = sees(p, |q| imports(q).collect());
        for (i, caller) in e.items.iter().enumerate() {
            let own = caller.owner.as_deref().map(owner_key);
            let mut calls: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
            for site in &caller.call_sites {
                for c in resolve(&index, site, own, p, &sees) {
                    if c != (p, i) {
                        let span = index.item(c).span(&index.unit(c.0).source_path);
                        calls.entry(span).or_default().insert(site.line);
                    }
                }
            }
            resolved.push((p, i, calls));
        }
    }
    let ids: Vec<String> = ids.iter().map(|s| (*s).to_string()).collect();
    for (p, i, calls) in resolved {
        let it = &mut entities.get_mut(&ids[p]).unwrap().items[i];
        it.call_lines = calls
            .values()
            .map(|l| l.iter().copied().collect())
            .collect();
        it.calls = calls.into_keys().collect();
        it.call_sites = Vec::new();
    }
}

/// A function that calls another, as `callers` shows it.
#[derive(Clone, Debug)]
pub struct Caller {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    /// `owner::name`, or `name`.
    pub name: String,
    pub test: bool,
    /// Its lines that make the call; empty in a wiki compiled before
    /// `call_lines`.
    pub lines: Vec<usize>,
}

impl Caller {
    pub fn span(&self) -> String {
        format!("{}:{}-{}", self.path, self.start_line, self.end_line)
    }
}

/// Every function's callers, keyed by its `path:start-end`: the `calls`
/// edges turned around, each list tests last, then by path and line.
pub(crate) fn callers_index<'a>(
    code: impl Iterator<Item = (&'a str, &'a [CodeItem])>,
) -> BTreeMap<String, Vec<Caller>> {
    let mut index: BTreeMap<String, Vec<Caller>> = BTreeMap::new();
    for (path, items) in code {
        for it in items {
            for (k, callee) in it.calls.iter().enumerate() {
                index.entry(callee.clone()).or_default().push(Caller {
                    path: path.to_string(),
                    start_line: it.start_line,
                    end_line: it.end_line,
                    name: it.qualified_name(),
                    test: it.test,
                    lines: it.call_lines.get(k).cloned().unwrap_or_default(),
                });
            }
        }
    }
    for list in index.values_mut() {
        list.sort_by(|a, b| (a.test, &a.path, a.start_line).cmp(&(b.test, &b.path, b.start_line)));
    }
    index
}

impl Wiki {
    /// What function `symbol` on page `id` calls, directly and through its
    /// callees, to `depth` calls deep (no limit when `None`): one line per
    /// function, grouped by file in the order the walk first reaches them.
    /// `symbol` is a name or `Owner::name`; every function on the page it
    /// names is a root. Over `max_tokens`, the farthest functions are cut
    /// and a last line says how many. `Err` lists the page's functions when
    /// none is `symbol`.
    pub fn calls(
        &self,
        id: &str,
        symbol: &str,
        depth: Option<usize>,
        max_tokens: Option<usize>,
    ) -> Result<String, String> {
        self.call_walk(id, symbol, depth, max_tokens, false)
    }

    /// The functions that call `symbol` on page `id`, and theirs, to
    /// `depth` levels (no limit when `None`), as `calls` lists callees;
    /// test code comes last.
    pub fn callers(
        &self,
        id: &str,
        symbol: &str,
        depth: Option<usize>,
        max_tokens: Option<usize>,
    ) -> Result<String, String> {
        self.call_walk(id, symbol, depth, max_tokens, true)
    }

    /// How many functions call item `it` on the page at `path`, as
    /// `callers` lists them one level up: its twin's callers count too, and
    /// neither half counts as its own caller.
    pub(crate) fn caller_count(&self, path: &str, it: &CodeItem) -> usize {
        let roots: Vec<String> = std::iter::once(it.span(path))
            .chain(it.twin.clone())
            .collect();
        let by: BTreeSet<String> = roots
            .iter()
            .flat_map(|s| self.callers_of(s).iter().map(Caller::span))
            .filter(|s| !roots.contains(s))
            .collect();
        by.len()
    }

    /// `calls`, or with `up` `callers`.
    fn call_walk(
        &self,
        id: &str,
        symbol: &str,
        depth: Option<usize>,
        max_tokens: Option<usize>,
        up: bool,
    ) -> Result<String, String> {
        let path = self.entry(id).map_or("", |e| e.path.as_str());
        let fns = || {
            self.items(id).iter().filter(|it| {
                matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method) && !it.nested
            })
        };
        let symbol = symbol.trim().trim_end_matches("()");
        let roots: Vec<&CodeItem> = fns()
            .filter(|it| it.name == symbol || it.qualified_name() == symbol)
            .collect();
        if roots.is_empty() {
            let mut names: Vec<String> = fns().map(CodeItem::qualified_name).collect();
            names.sort();
            names.dedup();
            names.truncate(20);
            let list = if names.is_empty() {
                "it has none".to_string()
            } else {
                format!("its functions: {}", names.join(", "))
            };
            return Err(format!("no function {symbol:?} on page {id}; {list}"));
        }
        let by_span: BTreeMap<String, (&str, &CodeItem)> = self
            .code_items()
            .flat_map(|(p, items)| items.iter().map(move |it| (it.span(p), (p, it))))
            .collect();
        // A declaration's calls are its definition's, so both are roots.
        let mut root_spans: Vec<String> = Vec::new();
        for it in &roots {
            let own = it.span(path);
            for s in std::iter::once(own).chain(it.twin.clone()) {
                if !root_spans.contains(&s) {
                    root_spans.push(s);
                }
            }
        }
        let mut queue: VecDeque<(String, usize)> =
            root_spans.iter().map(|s| (s.clone(), 0)).collect();
        let mut seen: BTreeSet<String> = queue.iter().map(|(s, _)| s.clone()).collect();
        let mut order: Vec<(&str, &CodeItem)> = Vec::new();
        // Up the walk: per caller reached, the call that reached it.
        let mut via: BTreeMap<String, String> = BTreeMap::new();
        while let Some((s, d)) = queue.pop_front() {
            let Some(&(p, it)) = by_span.get(&s) else {
                continue;
            };
            if d > 0 {
                order.push((p, it));
            }
            if depth.is_some_and(|max| d >= max) {
                continue;
            }
            let next: Vec<String> = if up {
                let callers = self.callers_of(&s);
                for c in callers.iter().filter(|c| !c.lines.is_empty()) {
                    let at: Vec<String> = c.lines.iter().map(usize::to_string).collect();
                    via.entry(c.span())
                        .or_insert_with(|| format!("  calls {} at {}", it.name, at.join(", ")));
                }
                callers.iter().map(Caller::span).collect()
            } else {
                it.calls.clone()
            };
            for c in next {
                if seen.insert(c.clone()) {
                    queue.push_back((c, d + 1));
                }
            }
        }
        if up {
            order.sort_by_key(|(_, it)| it.test);
        }

        let head = format!(
            "{} {}  {}",
            if up { "callers of" } else { "calls from" },
            roots[0].qualified_name(),
            root_spans.join(", ")
        );
        // Nothing seen calls it: say what the walk cannot see. The budget
        // never cuts this line.
        if up && order.is_empty() && depth != Some(0) {
            return Ok(format!(
                "{head} — 0 functions in 0 files (not seen: a call outside any function, a function passed as a value or called by a computed name, or a call from another language or an unindexed file; grep for {})\n",
                roots[0].name
            ));
        }
        let render = |n: usize| {
            let kept = &order[..n];
            let mut files: Vec<&str> = Vec::new();
            for (p, _) in kept {
                if !files.contains(p) {
                    files.push(p);
                }
            }
            let mut out = vec![format!("{head} — {n} functions in {} files", files.len())];
            for p in files {
                out.push(p.to_string());
                let mut lines: Vec<&CodeItem> = kept
                    .iter()
                    .filter(|(q, _)| *q == p)
                    .map(|(_, it)| *it)
                    .collect();
                lines.sort_by_key(|it| (it.start_line, it.end_line));
                for it in lines {
                    out.push(format!(
                        "  {}-{}  {}{}  {}{}",
                        it.start_line,
                        it.end_line,
                        it.kind.as_str(),
                        if it.test { " [test]" } else { "" },
                        it.qualified_name(),
                        via.get(&it.span(p)).map_or("", String::as_str)
                    ));
                }
            }
            out.join("\n") + "\n"
        };
        let full = render(order.len());
        let max = match max_tokens {
            Some(max) if token_estimate(&full) > max => max,
            _ => return Ok(full),
        };
        let total = token_estimate(&full);
        let marker = |n: usize| {
            format!(
                "… {} more functions — pass max_tokens: {total} for all\n",
                order.len() - n
            )
        };
        // The longest prefix of the walk that fits with its marker (more
        // lines never cost less); the header and the marker are kept even
        // when they do not fit.
        let cut = |n: usize| render(n) + &marker(n);
        Ok(cut(largest_fit(order.len(), |n| {
            token_estimate(&cut(n)) <= max
        })))
    }
}
