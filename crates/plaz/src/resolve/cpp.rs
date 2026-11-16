//! C and C++'s own call rules: the files the quoted includes reach,
//! functions local to their file, declarations folded onto their
//! definitions, and the classes a method call may dispatch to.

mod classes;
mod members;

use super::{Caller, Evidence};
use crate::flow::FlowScope;
use crate::model::{Bodiless, File, MapCall};
use classes::{Classes, Missing, Sight};
use members::this_field;
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::{owner_key, Family, FnRef};
use wiki_code::model::{CallSite, CallVia, CodeItem, CodeItemKind};
use wiki_code::support::source_rank;

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
    /// The functions by name, a function both trees list once; an
    /// operator's name without spaces (`operator->` of `operator -> ()`).
    by_name: BTreeMap<String, Vec<FnRef>>,
    /// The definitions no file keeps to itself, by key.
    shared: BTreeMap<Key<'a>, Vec<FnRef>>,
    /// Every definition, by page and key.
    own: BTreeMap<(usize, Key<'a>), Vec<FnRef>>,
    /// Every definition, by page, scope and name.
    file_defs: BTreeMap<(usize, &'a str, &'a str), Vec<FnRef>>,
    namespaces: BTreeSet<&'a str>,
    macros: BTreeSet<&'a str>,
    /// Type names, the names of the classes the heads define, and owners
    /// of out-of-line definitions that are no namespace or misread.
    types: BTreeSet<&'a str>,
    classes: Classes<'a>,
    /// The stems of the project's Objective-C and Objective-C++ files.
    unread: BTreeSet<&'a str>,
}

impl<'a> Cpp<'a> {
    pub fn new(files: &'a BTreeMap<String, File>, objc: &'a [String]) -> Cpp<'a> {
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
            file_defs: BTreeMap::new(),
            namespaces: cpp()
                .flat_map(|(_, (_, f))| &f.namespaces)
                .map(String::as_str)
                .collect(),
            macros: cpp()
                .flat_map(|(_, (_, f))| &f.macros)
                .map(|m| m.0.as_str())
                .collect(),
            types: BTreeSet::new(),
            classes: Classes::default(),
            unread: (objc.iter())
                .filter_map(|p| p.rsplit('/').next()?.rsplit_once('.'))
                .map(|s| s.0)
                .collect(),
            pages: pages.clone(),
        };
        for (page, &(path, file)) in cpp() {
            // `<a/b.h>` may name a project header; `<b.h>` a system one.
            let quoted = file
                .code
                .imports
                .iter()
                .filter_map(|i| match i.strip_prefix('<') {
                    Some(a) => a.strip_suffix('>').filter(|a| a.contains('/')),
                    None => Some(i.as_str()),
                });
            out.includes[page] = quoted
                .flat_map(|i| out.include(&by_file, path, i))
                .collect();
            for (i, it) in file.code.items.iter().enumerate() {
                if it.kind == CodeItemKind::Type {
                    out.types.insert(&it.name);
                }
                let owner = it.owner.as_deref().map(owner_key);
                let misread = it.kind == CodeItemKind::Method && !file.classes.contains_key(&i);
                out.types
                    .extend(owner.filter(|o| !misread && !out.namespaces.contains(o)));
                if !matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method) || it.nested {
                    continue;
                }
                let c = (page, i);
                // `operator -> ()` is `operator->`: an operator name with a symbol
                // loses its spaces (`operator new[]` too); `operator new` keeps them.
                let op = it.name.strip_prefix("operator");
                let op = op.filter(|o| o.contains(|c: char| c.is_ascii_punctuation()));
                let name = op.map_or_else(
                    || it.name.clone(),
                    |o| format!("operator{}", o.replace(' ', "")),
                );
                out.by_name.entry(name).or_default().push(c);
                match out.key(c) {
                    Some(key) if !it.test && !out.declared(c) => {
                        if !file.local.contains(&i) {
                            out.shared.entry(key).or_default().push(c);
                        }
                        out.own.entry((page, key)).or_default().push(c);
                        let named = out.file_defs.entry((page, key.0, key.1));
                        named.or_default().push(c);
                    }
                    Some(_) | None => {}
                }
            }
        }
        out.read_classes();
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

    fn is_c(&self, page: usize) -> bool {
        // `.C` is C++.
        std::path::Path::new(self.pages[page].0).extension() == Some("c".as_ref())
    }

    fn local(&self, c: FnRef) -> bool {
        self.pages[c.0].1.local.contains(&c.1)
    }

    /// Does a definition among `fns` stand in the file of `page`? A call
    /// there reaches its own unit's definition, whatever `folded` opened.
    fn defined_in(&self, fns: &BTreeSet<FnRef>, page: usize) -> bool {
        fns.iter().any(|&c| c.0 == page && !self.declared(c))
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
    /// own file when it is local, else those no file keeps to itself; with
    /// none, its file's one definition of that name (and arity, in C++); a
    /// declaration with neither stays. A free function's declarations none
    /// of whose definitions' files includes one (`extern int main(int,
    /// char**);` beside each program's `main`) may be any program's: each
    /// definition of that name and arity, and the set is open. So is it
    /// for a declaration in a header an unread Objective-C file shares a
    /// stem with (`pad.hpp`, `pad.mm`).
    fn folded(&self, fns: Vec<FnRef>) -> (BTreeSet<FnRef>, bool) {
        let mut out = BTreeSet::new();
        // Per free function's key: a declaration, and is one tied?
        let mut tied: BTreeMap<Key, (FnRef, bool)> = BTreeMap::new();
        let mut twin = false;
        for c in fns {
            let Some(key) = self.key(c).filter(|_| self.declared(c)) else {
                out.insert(c);
                continue;
            };
            // An unread Objective-C file of the header's stem may define it.
            let path = self.pages[c.0].0;
            let stem = path.rsplit('/').next().and_then(|f| f.rsplit_once('.'));
            twin |= source_rank(path) < 2 && stem.is_some_and(|s| self.unread.contains(s.0));
            let page = self.local(c).then_some(c.0);
            let defs = match page {
                Some(p) => self.own.get(&(p, key)),
                None => self.shared.get(&key),
            };
            if let Some(defs) = defs {
                out.extend(defs);
                if page.is_none() && self.item(c).kind == CodeItemKind::Fn {
                    let includes = |d: &FnRef| d.0 == c.0 || self.closure(d.0).contains(&c.0);
                    tied.entry(key).or_insert((c, false)).1 |= defs.iter().any(includes);
                }
                continue;
            }
            // Spelled otherwise: the file's one definition of that name, of
            // that arity in C++ (C has one signature).
            let named = self
                .file_defs
                .get(&(c.0, key.0, key.1))
                .into_iter()
                .flatten();
            let mut same =
                named.filter(|&&d| self.is_c(c.0) || self.item(d).arity == self.item(c).arity);
            match (same.next(), same.next()) {
                (Some(&d), None) => out.insert(d),
                _ => out.insert(c),
            };
        }
        let loose: Vec<(Key, FnRef)> = (tied.into_iter())
            .filter(|t| !t.1 .1)
            .map(|(key, (c, _))| (key, c))
            .collect();
        for &((scope, name, _), c) in &loose {
            let named = (self.shared.range((scope, name, "")..))
                .take_while(|(k, _)| (k.0, k.1) == (scope, name));
            let defs = named.flat_map(|(_, d)| d);
            out.extend(defs.filter(|&&d| self.item(d).arity == self.item(c).arity));
        }
        (out, twin || !loose.is_empty())
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
    /// The target of a call in a C/C++ function.
    pub(super) fn cpp(&self, chain: &[FlowScope], call: &MapCall) -> Evidence {
        match &call.site.via {
            CallVia::Bare => self.cpp_bare(chain, call),
            CallVia::Path(q) => self.cpp_path(chain, call, q),
            CallVia::SelfRecv => match self.member_of(chain) {
                Some(class) => self.on_class(call, class),
                None => self.any_method(call),
            },
            // The innermost flow binding `x` gives its type; with none, a
            // data member of that name.
            CallVia::Member(Some(x)) => {
                let after = call.text.strip_prefix(x.as_str()).unwrap_or("");
                match chain.iter().rev().find(|f| f.locals.contains(x)) {
                    Some(f) => match f.types.get(x) {
                        Some(t) => self.on_typed(
                            call,
                            after,
                            t,
                            self.class.unwrap_or(self.ns),
                            self.sight(),
                        ),
                        None => self.any_method(call),
                    },
                    None => self.on_field(chain, call, x, Some(after)),
                }
            }
            CallVia::Member(None) => match this_field(&call.text, &call.site.name) {
                Some((x, after)) => self.on_field(chain, call, x, Some(after)),
                None => self.any_method(call),
            },
        }
    }

    /// The class whose members a name in the innermost flow of `chain`
    /// may be: the caller's, unless a local class's method nested in the
    /// caller holds it (the caller's own flow is an item, never one).
    fn member_of(&self, chain: &[FlowScope]) -> Option<&str> {
        self.class
            .filter(|_| !chain.iter().skip(1).any(|f| f.local_class))
    }

    /// What the caller's file sees.
    fn sight(&self) -> Sight<'_> {
        Sight {
            page: self.page,
            included: self.included,
        }
    }

    /// `q::f()`.
    fn cpp_path(&self, chain: &[FlowScope], call: &MapCall, q: &str) -> Evidence {
        let (cpp, site) = (self.cpp, &call.site);
        // `Box<int>` of `Box<int>::f()` is the type `Box`.
        let root = call.root.as_deref().map_or(q, owner_key);
        // `fs::f()` under `namespace fs = std::filesystem;` (`q` is then
        // `filesystem`) starts at `std`.
        let mut pages = std::iter::once(&self.page).chain(self.included);
        let alias = pages.find_map(|&p| cpp.pages[p].1.namespace_aliases.get(root));
        let root = alias.map_or(root, |a| a.split("::").next().unwrap_or(""));
        let project =
            cpp.types.contains(root) || cpp.namespaces.contains(root) || self.index.is_module(root);
        if (chain.iter()).any(|f| f.rebound.contains(root)) {
            // A template parameter or a local type: any type's method.
            self.any_method(call)
        } else if root == "std" || !project {
            Evidence::Outside
        } else if cpp.types.contains(q) {
            self.on_qualifier(call, q)
        } else if cpp.namespaces.contains(q) {
            self.free(site, Some(q)).unwrap_or(Evidence::Hidden)
        } else {
            Evidence::Hidden
        }
    }

    /// `f()`: a binding, a member of the caller's class, else a free
    /// function, a macro or type, or something outside.
    fn cpp_bare(&self, chain: &[FlowScope], call: &MapCall) -> Evidence {
        let site = &call.site;
        let name = site.name.as_str();
        // A local called is its own call operator.
        if let Some(f) = chain.iter().rev().find(|f| f.locals.contains(name)) {
            return match f.types.get(name) {
                Some(t) => self.called(call, t, self.class.unwrap_or(self.ns), self.sight()),
                None => Evidence::Hidden,
            };
        }
        if (chain.iter()).any(|f| f.rebound.contains(name)) {
            return Evidence::Hidden;
        }
        // `::f()` names no member.
        if let Some(class) = self.class.filter(|_| !call.text.starts_with("::")) {
            // In a local class's method the enclosing class's members are
            // in reach, but the local class's own may hide them.
            let local = self.member_of(chain).is_none();
            // A data member called is its own call operator.
            if !local && self.cpp.field(class, name, self.sight()).is_some() {
                return self.on_field(chain, call, name, None);
            }
            // A member the class may inherit hides the free functions; one
            // of a class no head shows may be any method.
            let or_free = |or: Evidence| match (self.free(site, None), or) {
                (Some(Evidence::Found { mut fns, .. }), or) => {
                    if let Evidence::Found { fns: more, .. } = or {
                        fns.extend(more);
                    }
                    Evidence::Found { fns, open: true }
                }
                (Some(_) | None, or) => or,
            };
            match self.cpp.lookup(class, site, self.sight(), true) {
                // A bare call in a template does not search a dependent
                // base: the free functions may be meant.
                Ok(found) if found.dependent || local => {
                    let mut fns = self.ids(found.fns);
                    return match self.free(site, None) {
                        Some(Evidence::Found { fns: free, open }) => {
                            fns.extend(free);
                            Evidence::Found {
                                fns,
                                open: open || local,
                            }
                        }
                        Some(_) | None => Evidence::Found { fns, open: true },
                    };
                }
                Ok(found) => {
                    return Evidence::Found {
                        fns: self.ids(found.fns),
                        open: found.open,
                    }
                }
                Err(Missing::Outside) if !local => return or_free(Evidence::Outside),
                Err(Missing::Unseen) if !local => return or_free(self.any_method(call)),
                Err(Missing::Outside | Missing::Unseen | Missing::Nowhere) => {}
            }
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
    /// in C those of the caller's file first; those of the caller's file
    /// and the files it includes; else any other not local to its file (in
    /// a header, called from a header, it may be), which may also be
    /// something outside. A bare call's set keeps the functions in view,
    /// or with none in view is open. None by argument count: the same over
    /// the variadic ones.
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
            // A header's file-local function may be in every unit that
            // includes the calling header too.
            let header = |page: usize| source_rank(cpp.pages[page].0) < 2;
            let reachable = |c: FnRef| !cpp.local(c) || (header(self.page) && header(c.0));
            let own = |c: FnRef| c.0 == self.page;
            // In C a name has one signature: the caller's own is the one.
            let c_file = cpp.is_c(self.page);
            let tiers = [
                (tier(&|c| c_file && own(c)), false),
                (tier(&|c| own(c) || seen(c)), false),
                (tier(&|c| !own(c) && !seen(c) && reachable(c)), true),
            ];
            for (fns, open) in tiers {
                // A bare call sees a namespace's function from in it or
                // through a `using`; argument-dependent lookup, any.
                let shown: Vec<FnRef> = (fns.iter().copied())
                    .filter(|&c| ns.is_some() || self.in_view(c, &site.name))
                    .collect();
                let (fns, open) = if shown.is_empty() {
                    (fns, true)
                } else {
                    (shown, open)
                };
                let (fns, loose) = cpp.folded(fns);
                let loose = loose && !cpp.defined_in(&fns, self.page);
                let fns = self.ids(fns);
                if !fns.is_empty() {
                    return Some(Evidence::Found {
                        fns,
                        open: open || loose,
                    });
                }
            }
        }
        None
    }

    /// Is the function `c` in view of a bare call of `name` here: in no
    /// namespace, in the caller's or an enclosing one, or named by one of
    /// the file's `using`s?
    fn in_view(&self, c: FnRef, name: &str) -> bool {
        let at = self.cpp.pages[c.0]
            .1
            .scopes
            .get(&c.1)
            .map_or("", String::as_str);
        let encloses = self.ns == at || self.ns.starts_with(&format!("{at}::"));
        let names = |u: &str| at == u || at.ends_with(&format!("::{u}"));
        let using = |u: &String| {
            let of = u.strip_suffix(name).and_then(|n| n.strip_suffix("::"));
            names(u) || of.is_some_and(names)
        };
        at.is_empty() || encloses || self.file.usings.iter().any(using)
    }
}
