//! C and C++'s own call rules for `f()` and `q::f()`: the files the quoted
//! includes reach, functions local to their file, and declarations folded
//! onto their definitions.

use super::{Caller, Evidence};
use crate::flow::FlowScope;
use crate::model::{Bodiless, File, MapCall};
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::{owner_key, Family, FnRef};
use wiki_code::model::{CallSite, CallVia, CodeItem, CodeItemKind};

/// What a declaration shares with its definition: the owner key, else the
/// namespace (`core` both of `int core::run(int)` and of a `run` declared
/// in `namespace core`); the name; the parameter types.
type Key<'a> = (&'a str, &'a str, &'a str);

/// What the project's C/C++ files say before any call is looked at. A page
/// is a position in the project's key order, as in the index.
pub(super) struct Cpp<'a> {
    pages: Vec<(&'a str, &'a File)>,
    /// Per page: the pages its quoted includes name.
    includes: Vec<Vec<usize>>,
    /// The functions by name, a function both trees list once.
    by_name: BTreeMap<&'a str, Vec<FnRef>>,
    /// The definitions no file keeps to itself, by key.
    shared: BTreeMap<Key<'a>, Vec<FnRef>>,
    /// Every definition, by page and key.
    own: BTreeMap<(usize, Key<'a>), Vec<FnRef>>,
    namespaces: BTreeSet<&'a str>,
    macros: BTreeSet<&'a str>,
    /// Type names, and owners of out-of-line definitions that are no
    /// namespace.
    types: BTreeSet<&'a str>,
}

impl<'a> Cpp<'a> {
    pub fn new(files: &'a BTreeMap<String, File>) -> Cpp<'a> {
        let pages: Vec<(&str, &File)> = files.iter().map(|(p, f)| (p.as_str(), f)).collect();
        let cpp = || {
            let all = pages.iter().enumerate();
            all.filter(|(_, (_, f))| f.family == Family::Cpp)
        };
        let mut by_file: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (page, (path, _)) in cpp() {
            by_file
                .entry(path.rsplit('/').next().unwrap_or(path))
                .or_default()
                .push(page);
        }
        let mut out = Cpp {
            includes: vec![Vec::new(); pages.len()],
            by_name: BTreeMap::new(),
            shared: BTreeMap::new(),
            own: BTreeMap::new(),
            namespaces: cpp()
                .flat_map(|(_, (_, f))| &f.namespaces)
                .map(String::as_str)
                .collect(),
            macros: cpp()
                .flat_map(|(_, (_, f))| &f.macros)
                .map(|m| m.0.as_str())
                .collect(),
            types: BTreeSet::new(),
            pages: pages.clone(),
        };
        for (page, &(path, file)) in cpp() {
            let quoted = file.code.imports.iter().filter(|i| !i.starts_with('<'));
            out.includes[page] = quoted
                .flat_map(|i| out.include(&by_file, path, i))
                .collect();
            for (i, it) in file.code.items.iter().enumerate() {
                if it.kind == CodeItemKind::Type {
                    out.types.insert(&it.name);
                }
                let owner = it.owner.as_deref().map(owner_key);
                out.types
                    .extend(owner.filter(|o| !out.namespaces.contains(o)));
                if !matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method) || it.nested {
                    continue;
                }
                let c = (page, i);
                out.by_name.entry(&it.name).or_default().push(c);
                match out.key(c) {
                    Some(key) if !it.test && !out.declared(c) => {
                        if !file.local.contains(&i) {
                            out.shared.entry(key).or_default().push(c);
                        }
                        out.own.entry((page, key)).or_default().push(c);
                    }
                    Some(_) | None => {}
                }
            }
        }
        out
    }

    /// The pages `#include "inc"` in the file at `from` names: the file
    /// at that path relative to `from`'s directory, else every file whose
    /// path ends in it (`..` segments dropped).
    fn include(&self, by_file: &BTreeMap<&str, Vec<usize>>, from: &str, inc: &str) -> Vec<usize> {
        let mut dir: Vec<&str> = from.split('/').collect();
        dir.pop();
        let mut inside = true;
        for seg in inc.split('/') {
            match seg {
                "" | "." => {}
                ".." => inside &= dir.pop().is_some(),
                s => dir.push(s),
            }
        }
        let want = dir.join("/");
        let named = by_file.get(inc.rsplit('/').next().unwrap_or(inc));
        let named = named.map_or(&[][..], Vec::as_slice);
        if let Some(&p) = named.iter().find(|&&p| inside && self.pages[p].0 == want) {
            return vec![p];
        }
        let segs = inc.split('/').skip_while(|s| matches!(*s, "" | "." | ".."));
        let tail = segs.collect::<Vec<_>>().join("/");
        let ends = |p: &str| p == tail || p.ends_with(&format!("/{tail}"));
        named
            .iter()
            .copied()
            .filter(|&p| ends(self.pages[p].0))
            .collect()
    }

    /// The pages the file of `page` includes, directly or through another.
    pub fn closure(&self, page: usize) -> BTreeSet<usize> {
        let mut seen = BTreeSet::new();
        let mut todo = self.includes[page].clone();
        while let Some(p) = todo.pop() {
            if seen.insert(p) {
                todo.extend(&self.includes[p]);
            }
        }
        seen
    }

    fn item(&self, c: FnRef) -> &'a CodeItem {
        &self.pages[c.0].1.code.items[c.1]
    }

    fn local(&self, c: FnRef) -> bool {
        self.pages[c.0].1.local.contains(&c.1)
    }

    fn declared(&self, c: FnRef) -> bool {
        self.pages[c.0].1.declarations.get(&c.1) == Some(&Bodiless::Declared)
    }

    fn key(&self, c: FnRef) -> Option<Key<'a>> {
        let it = self.item(c);
        let owner = it.owner.as_deref().map(owner_key);
        let scope = owner.or(it.namespace.as_deref()).unwrap_or("");
        let params = self.pages[c.0].1.params.get(&c.1)?;
        Some((scope, it.name.as_str(), params.as_str()))
    }

    /// `fns`, each declaration taken as the definitions of its key: of its
    /// own file when it is local, else those no file keeps to itself; a
    /// declaration with none stays.
    fn folded(&self, fns: Vec<FnRef>) -> BTreeSet<FnRef> {
        let mut out = BTreeSet::new();
        let mut done = BTreeSet::new();
        for c in fns {
            let Some(key) = self.key(c).filter(|_| self.declared(c)) else {
                out.insert(c);
                continue;
            };
            let page = self.local(c).then_some(c.0);
            let defs = match page {
                Some(p) => self.own.get(&(p, key)),
                None => self.shared.get(&key),
            };
            match defs {
                Some(defs) if done.insert((page, key)) => out.extend(defs),
                Some(_) => {}
                None => {
                    out.insert(c);
                }
            }
        }
        out
    }
}

/// Can `it` take `site`'s argument count?
fn fits(it: &CodeItem, site: &CallSite) -> bool {
    match (it.arity, site.args) {
        (Some(a), Some(n)) => a.fits(n, false),
        _ => true,
    }
}

impl Caller<'_, '_> {
    /// The target of `f()` or `q::f()` in a C/C++ function.
    pub(super) fn cpp(&self, chain: &[FlowScope], call: &MapCall) -> Evidence {
        let (cpp, site) = (self.cpp, &call.site);
        let CallVia::Path(q) = &site.via else {
            return self.cpp_bare(chain, site);
        };
        let q = q.as_str();
        let root = call.root.as_deref().unwrap_or(q);
        let project =
            cpp.types.contains(root) || cpp.namespaces.contains(root) || self.index.is_module(root);
        if root == "std" || !project {
            Evidence::Outside
        } else if cpp.types.contains(q) {
            // `Base::f()` names that one function: no dispatch.
            let named = cpp.by_name.get(site.name.as_str()).into_iter().flatten();
            let members = named.copied().filter(|&c| {
                let it = cpp.item(c);
                it.kind == CodeItemKind::Method
                    && it.owner.as_deref().map(owner_key) == Some(q)
                    && fits(it, site)
            });
            Evidence::Found {
                fns: self.ids(cpp.folded(members.collect())),
                open: false,
            }
        } else if cpp.namespaces.contains(q) {
            self.free(site, Some(q)).unwrap_or(Evidence::Hidden)
        } else {
            Evidence::Hidden
        }
    }

    fn cpp_bare(&self, chain: &[FlowScope], site: &CallSite) -> Evidence {
        let name = site.name.as_str();
        if (chain.iter()).any(|f| f.locals.contains(name) || f.rebound.contains(name)) {
            return Evidence::Hidden;
        }
        // Until the class rules: a method's call may name a member.
        if self.item.kind == CodeItemKind::Method && !self.scope.methods_anywhere(site).is_empty() {
            return Evidence::Hidden;
        }
        if let Some(found) = self.free(site, None) {
            return found;
        }
        if self.cpp.macros.contains(name) || self.cpp.types.contains(name) {
            Evidence::Hidden
        } else {
            Evidence::Outside
        }
    }

    /// The free functions `site` names, of the namespace `ns` when given:
    /// those of the caller's file, else of the files it includes, else any
    /// other not local to its file, which may also be something outside.
    /// None by argument count: the same over the variadic ones.
    fn free(&self, site: &CallSite, ns: Option<&str>) -> Option<Evidence> {
        let cpp = self.cpp;
        let in_ns = |it: &CodeItem| match ns {
            None => it.kind == CodeItemKind::Fn,
            Some(q) => match it.kind {
                CodeItemKind::Fn => it.namespace.as_deref() == Some(q),
                // `int core::run(int)` is `run` of the namespace `core`.
                CodeItemKind::Method => it.owner.as_deref().map(owner_key) == Some(q),
                CodeItemKind::Type
                | CodeItemKind::Value
                | CodeItemKind::Module
                | CodeItemKind::TopLevel => false,
            },
        };
        let named = cpp
            .by_name
            .get(site.name.as_str())
            .map_or(&[][..], Vec::as_slice);
        let by_arity = |it: &CodeItem| fits(it, site);
        let variadic = |it: &CodeItem| it.signature.contains("...");
        for keep in [&by_arity as &dyn Fn(&CodeItem) -> bool, &variadic] {
            let all: Vec<FnRef> = (named.iter().copied())
                .filter(|&c| in_ns(cpp.item(c)) && keep(cpp.item(c)))
                .collect();
            let tier = |pick: &dyn Fn(FnRef) -> bool| -> Vec<FnRef> {
                all.iter().copied().filter(|&c| pick(c)).collect()
            };
            let seen = |c: FnRef| self.included.contains(&c.0);
            let tiers = [
                (tier(&|c| c.0 == self.page), false),
                (tier(&|c| c.0 != self.page && seen(c)), false),
                (
                    tier(&|c| c.0 != self.page && !seen(c) && !cpp.local(c)),
                    true,
                ),
            ];
            for (fns, open) in tiers {
                let fns = self.ids(cpp.folded(fns));
                if !fns.is_empty() {
                    return Some(Evidence::Found { fns, open });
                }
            }
        }
        None
    }
}
