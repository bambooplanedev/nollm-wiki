//! Call edges between code items: matched by name at compile time
//! (`resolve_calls`), followed at query time (`Wiki::calls`).
//!
//! Matching is by name alone, with no type inference, so the edges are the
//! same on every run. A name with several matches keeps them all: a method
//! called on a value of unknown type links every same-named method the
//! caller's file can see.

use crate::formats::code::{module_stem, source_rank};
use crate::manifest::token_estimate;
use crate::model::{CallSite, CallVia, CodeItem, CodeItemKind, Entity, Graph, SourceKind};
use crate::query::Wiki;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The last segment of an owner, generics dropped: `Outer.Inner` and
/// `a::Inner<T>` are both `Inner`.
fn owner_key(owner: &str) -> &str {
    let head = owner.split('<').next().unwrap_or("");
    head.rsplit([':', '.']).next().unwrap_or("").trim()
}

/// The languages whose code calls each other: a page's code language, with
/// every JavaScript dialect and component format as one.
fn family(e: &Entity) -> &str {
    match &e.kind {
        SourceKind::Code { lang } if matches!(lang.as_str(), "rust" | "python" | "cpp" | "go") => {
            lang
        }
        _ => "ecma",
    }
}

/// A function item: (page, item) indexes into `Index::pages`.
type FnRef = (usize, usize);

/// What call sites resolve against: every function by name, every page by
/// module name, every type name.
struct Index<'a> {
    pages: Vec<&'a Entity>,
    by_name: BTreeMap<&'a str, Vec<FnRef>>,
    /// Pages by module stem and by directory name.
    modules: BTreeMap<&'a str, BTreeSet<usize>>,
    /// Owner last segments and type item names.
    types: BTreeSet<&'a str>,
    /// A header declaration's definition: its `twin`, else the one function
    /// of the same name and owner in a file of the same stem.
    def_of: BTreeMap<FnRef, FnRef>,
}

impl<'a> Index<'a> {
    fn new(entities: &'a BTreeMap<String, Entity>) -> Self {
        let pages: Vec<&Entity> = entities.values().collect();
        let mut by_name: BTreeMap<&str, Vec<FnRef>> = BTreeMap::new();
        let mut by_span: BTreeMap<String, FnRef> = BTreeMap::new();
        let mut modules: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
        let mut types: BTreeSet<&str> = BTreeSet::new();
        for (p, e) in pages.iter().enumerate() {
            if e.items.is_empty() {
                continue;
            }
            let path = e.source_path.as_str();
            modules.entry(module_stem(path)).or_default().insert(p);
            if let Some(dir) = path.rsplit('/').nth(1) {
                modules.entry(dir).or_default().insert(p);
            }
            for (i, it) in e.items.iter().enumerate() {
                types.extend(it.owner.as_deref().map(owner_key));
                if it.kind == CodeItemKind::Type {
                    types.insert(&it.name);
                }
                if matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method) {
                    by_name.entry(&it.name).or_default().push((p, i));
                    by_span.insert(it.span(&e.source_path), (p, i));
                }
            }
        }
        let header = |c: FnRef| source_rank(&pages[c.0].source_path) < 2;
        let key = |c: FnRef| {
            let it = &pages[c.0].items[c.1];
            let stem = module_stem(&pages[c.0].source_path);
            (it.owner.as_deref().map(owner_key), stem)
        };
        let mut def_of = BTreeMap::new();
        for fns in by_name.values() {
            for &h in fns.iter().filter(|&&c| header(c)) {
                let twin = pages[h.0].items[h.1].twin.as_ref();
                let def = twin.and_then(|t| by_span.get(t).copied()).or_else(|| {
                    let mut defs = fns.iter().filter(|&&c| !header(c) && key(c) == key(h));
                    match (defs.next(), defs.next()) {
                        (Some(&d), None) => Some(d),
                        _ => None,
                    }
                });
                def_of.extend(def.map(|d| (h, d)));
            }
        }
        Index {
            pages,
            by_name,
            modules,
            types,
            def_of,
        }
    }

    fn item(&self, c: FnRef) -> &'a CodeItem {
        &self.pages[c.0].items[c.1]
    }

    /// The functions named `name` in the language of page `page` that pass
    /// `keep`, each declaration taken as its definition, deduplicated.
    fn pick(&self, page: usize, name: &str, keep: impl Fn(FnRef) -> bool) -> Vec<FnRef> {
        let named = self.by_name.get(name).map_or(&[][..], Vec::as_slice);
        let lang = family(self.pages[page]);
        let found: BTreeSet<FnRef> = named
            .iter()
            .copied()
            .filter(|&c| family(self.pages[c.0]) == lang && keep(c))
            .map(|c| self.def_of.get(&c).copied().unwrap_or(c))
            .collect();
        found.into_iter().collect()
    }

    /// The functions one call site names, from a function owned by `own`
    /// on page `page`, which sees the pages `sees`; see `resolve_calls`.
    fn resolve(
        &self,
        site: &CallSite,
        own: Option<&str>,
        page: usize,
        sees: &BTreeSet<usize>,
    ) -> Vec<FnRef> {
        let name = site.name.as_str();
        let is_fn = |c: FnRef| self.item(c).kind == CodeItemKind::Fn;
        let is_method = |c: FnRef| self.item(c).kind == CodeItemKind::Method;
        let owned_by = |key: &str, c: FnRef| {
            is_method(c) && self.item(c).owner.as_deref().map(owner_key) == Some(key)
        };
        let only = |v: Vec<FnRef>| if v.len() == 1 { v } else { Vec::new() };
        let own_methods = || {
            own.map(|o| self.pick(page, name, |c| owned_by(o, c)))
                .unwrap_or_default()
        };
        let fn_here = || self.pick(page, name, |c| is_fn(c) && c.0 == page);
        let fn_seen = || self.pick(page, name, |c| is_fn(c) && sees.contains(&c.0));
        let method_seen = || self.pick(page, name, |c| is_method(c) && sees.contains(&c.0));
        let fn_only = || only(self.pick(page, name, is_fn));
        let method_only = || only(self.pick(page, name, is_method));
        let qualified = |q: &str| {
            let in_module = self.modules.get(q);
            self.pick(page, name, |c| {
                owned_by(q, c) || (is_fn(c) && in_module.is_some_and(|m| m.contains(&c.0)))
            })
        };
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
            CallVia::Member(Some(x))
                if self.types.contains(x.as_str()) || self.modules.contains_key(x.as_str()) =>
            {
                let q = || qualified(x.as_str());
                first(&[&q, &method_seen, &method_only])
            }
            CallVia::Member(_) => first(&[&method_seen, &method_only]),
        }
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
///   segment is `Q`) or a module (a file whose stem or directory is `Q`). A
///   `Q` that is neither is outside the project.
/// - `self.f()`: the caller's owner's; a method of a file it sees; the one
///   method of that name.
/// - `x.f()`: as `x::f()` when `x` names a project type or module; a method
///   of a file the caller sees; the one method of that name.
///
/// Only functions in the caller's language family (`family`) count. The
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
    let mut resolved: Vec<(usize, usize, Vec<String>)> = Vec::new();
    for (p, e) in index.pages.iter().enumerate() {
        if e.items.iter().all(|it| it.call_sites.is_empty()) {
            continue;
        }
        let mut sees: BTreeSet<usize> = imports(p).collect();
        sees.extend(sees.clone().into_iter().flat_map(imports));
        sees.insert(p);
        for (i, caller) in e.items.iter().enumerate() {
            let own = caller.owner.as_deref().map(owner_key);
            let calls: BTreeSet<String> = caller
                .call_sites
                .iter()
                .flat_map(|site| index.resolve(site, own, p, &sees))
                .filter(|&c| c != (p, i))
                .map(|c| index.item(c).span(&index.pages[c.0].source_path))
                .collect();
            resolved.push((p, i, calls.into_iter().collect()));
        }
    }
    let ids: Vec<String> = ids.iter().map(|s| (*s).to_string()).collect();
    for (p, i, calls) in resolved {
        let it = &mut entities.get_mut(&ids[p]).unwrap().items[i];
        it.calls = calls;
        it.call_sites = Vec::new();
    }
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
        let path = self.entry(id).map_or("", |e| e.path.as_str());
        let fns = || {
            self.items(id)
                .iter()
                .filter(|it| matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method))
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
            for c in &it.calls {
                if seen.insert(c.clone()) {
                    queue.push_back((c.clone(), d + 1));
                }
            }
        }

        let head = format!(
            "calls from {}  {}",
            roots[0].qualified_name(),
            root_spans.join(", ")
        );
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
                        "  {}-{}  {}{}  {}",
                        it.start_line,
                        it.end_line,
                        it.kind.as_str(),
                        if it.test { " [test]" } else { "" },
                        it.qualified_name()
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
        // The longest prefix of the walk that fits with its marker, by
        // bisection (more lines never cost less); the header and the marker
        // are kept even when they do not fit.
        let cut = |n: usize| render(n) + &marker(n);
        let (mut lo, mut hi) = (0, order.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if token_estimate(&cut(mid)) <= max {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        Ok(cut(lo))
    }
}
