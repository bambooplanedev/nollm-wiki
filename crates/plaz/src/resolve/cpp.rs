//! C and C++'s own call rules: the files the quoted includes reach,
//! functions local to their file, declarations folded onto their
//! definitions, and the classes a method call may dispatch to.

mod classes;

use super::{Caller, Evidence};
use crate::flow::FlowScope;
use crate::model::{Bodiless, File, MapCall};
use crate::source::untemplated;
use classes::{Classes, Missing};
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
    /// The functions by name, a function both trees list once.
    by_name: BTreeMap<&'a str, Vec<FnRef>>,
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
                out.by_name.entry(&it.name).or_default().push(c);
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
    /// declaration with neither stays.
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
                    // Spelled otherwise: the file's one definition of that
                    // name, of that arity in C++ (C has one signature).
                    let named = self
                        .file_defs
                        .get(&(c.0, key.0, key.1))
                        .into_iter()
                        .flatten();
                    let mut same = named
                        .filter(|&&d| self.is_c(c.0) || self.item(d).arity == self.item(c).arity);
                    match (same.next(), same.next()) {
                        (Some(&d), None) => out.insert(d),
                        _ => out.insert(c),
                    };
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
    /// The target of a call in a C/C++ function.
    pub(super) fn cpp(&self, chain: &[FlowScope], call: &MapCall) -> Evidence {
        match &call.site.via {
            CallVia::Bare => self.cpp_bare(chain, call),
            CallVia::Path(q) => self.cpp_path(chain, call, q),
            CallVia::SelfRecv => match self.class {
                Some(class) => self.on_class(call, class),
                None => self.any_method(call),
            },
            // The innermost flow binding `x` gives its type.
            CallVia::Member(Some(x)) => {
                let binder = chain.iter().rev().find(|f| f.locals.contains(x));
                match binder.and_then(|f| f.types.get(x)) {
                    Some(t) => self.on_typed(call, x, t),
                    None => self.any_method(call),
                }
            }
            CallVia::Member(None) => self.any_method(call),
        }
    }

    /// A call of `f` on an object of the project class `class`: what
    /// lookup finds, else outside under a base from outside the project,
    /// else any project method `f`.
    fn on_class(&self, call: &MapCall, class: &str) -> Evidence {
        match self.cpp.lookup(class, &call.site) {
            Ok((fns, open)) => Evidence::Found {
                fns: self.ids(fns),
                open,
            },
            Err(Missing::Outside) => Evidence::Outside,
            Err(Missing::Nowhere | Missing::Unseen) => self.any_method(call),
        }
    }

    /// `x.f()` or `x->f()` on `x` of the written type `t`. A smart pointer
    /// is itself with `.`, its pointee with `->`. A project class, found
    /// from the caller's class or namespace: as `on_class`. A project type
    /// no head places: any project method `f`. Any other type: the project
    /// classes deriving from it that define `f`, or something outside.
    fn on_typed(&self, call: &MapCall, x: &str, t: &str) -> Evidence {
        let (cpp, site) = (self.cpp, &call.site);
        let (head, arg) = t.split_once('<').unwrap_or((t, ""));
        let head = head.trim();
        let t = match head.strip_prefix("std::").unwrap_or(head) {
            "unique_ptr" | "shared_ptr" => {
                let rest = call.text.strip_prefix(x).unwrap_or("");
                if !rest.trim_start().starts_with("->") {
                    return Evidence::Outside;
                }
                let arg = arg.rsplit_once('>').map_or(arg, |a| a.0);
                arg.split(',').next().unwrap_or("").trim()
            }
            _ => t,
        };
        match cpp.resolve(t, self.class.unwrap_or(self.ns)) {
            Some(class) => self.on_class(call, class),
            None if cpp.is_type(t) => self.any_method(call),
            None => Evidence::Found {
                fns: self.ids(cpp.deriving(t, site)),
                open: true,
            },
        }
    }

    /// `q::f()`.
    fn cpp_path(&self, chain: &[FlowScope], call: &MapCall, q: &str) -> Evidence {
        let (cpp, site) = (self.cpp, &call.site);
        // `Box<int>` of `Box<int>::f()` is the type `Box`.
        let root = call.root.as_deref().map_or(q, owner_key);
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

    /// `Q::f()` with `Q` a project type names that one function, no
    /// dispatch: the member of the class `Q` names from the caller's
    /// scope; with several classes it may name, one of theirs, never
    /// certain; with no class head, the functions of a namespace `Q`, else
    /// the methods of an owner `Q`, or with none (an alias) any project
    /// method `f`, or something outside.
    fn on_qualifier(&self, call: &MapCall, q: &str) -> Evidence {
        let (cpp, site) = (self.cpp, &call.site);
        let callee = call.text.split('(').next().unwrap_or("");
        let callee = untemplated(callee);
        let written = callee.rsplit_once("::").map_or(q, |w| w.0);
        if let Some(class) = cpp.resolve(written, self.class.unwrap_or(self.ns)) {
            return Evidence::Found {
                fns: self.ids(cpp.members(class, site)),
                open: false,
            };
        }
        let namesakes = cpp.namesakes(written);
        if namesakes.is_empty() && cpp.namespaces.contains(q) {
            return self.free(site, Some(q)).unwrap_or(Evidence::Hidden);
        }
        if namesakes.is_empty() {
            let named = cpp.by_name.get(site.name.as_str()).into_iter().flatten();
            let members = named.copied().filter(|&c| {
                let it = cpp.item(c);
                it.kind == CodeItemKind::Method
                    && it.owner.as_deref().map(owner_key) == Some(q)
                    && fits(it, site)
            });
            // An alias may name any class.
            let fns = self.ids(cpp.folded(members.collect()));
            if fns.is_empty() {
                return self.any_method(call);
            }
            return Evidence::Found { fns, open: true };
        }
        let fns = self.ids(namesakes.into_iter().flat_map(|c| cpp.members(c, site)));
        // One member of several classes it may name: maybe another's.
        Evidence::Found {
            open: fns.len() == 1,
            fns,
        }
    }

    /// `f()`: a binding, a member of the caller's class, else a free
    /// function, a macro or type, or something outside.
    fn cpp_bare(&self, chain: &[FlowScope], call: &MapCall) -> Evidence {
        let site = &call.site;
        let name = site.name.as_str();
        if (chain.iter()).any(|f| f.locals.contains(name) || f.rebound.contains(name)) {
            return Evidence::Hidden;
        }
        // `::f()` names no member.
        if let Some(class) = self.class.filter(|_| !call.text.starts_with("::")) {
            // A member the class may inherit hides the free functions.
            let or_free = |or: Evidence| match self.free(site, None) {
                Some(Evidence::Found { fns, .. }) => Evidence::Found { fns, open: true },
                Some(_) | None => or,
            };
            match self.cpp.lookup(class, site) {
                Ok((fns, open)) => {
                    return Evidence::Found {
                        fns: self.ids(fns),
                        open,
                    }
                }
                Err(Missing::Outside) => return or_free(Evidence::Outside),
                Err(Missing::Unseen) => return or_free(self.any_method(call)),
                Err(Missing::Nowhere) => {}
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
                let fns = self.ids(cpp.folded(fns));
                if !fns.is_empty() {
                    return Some(Evidence::Found { fns, open });
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
