//! C and C++'s own call rules: the files the includes reach (quoted
//! ones, and angle ones naming a directory),
//! functions local to their file, declarations folded onto their
//! definitions, and the classes a method call may dispatch to.

mod classes;
mod fold;
mod free;
mod members;

use super::{Caller, Evidence};
use crate::flow::FlowScope;
use crate::model::{Bodiless, File, FnId, MapCall};
use crate::source::untemplated;
use classes::{Classes, Missing, Sight};
pub(crate) use fold::{fold_scope, namespace_owners};
use fold::{Key, Loose};
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::{owner_key, Family, FnRef};
use wiki_code::model::{CallSite, CallVia, CodeItem, CodeItemKind};

/// What the project's C/C++ files say before any call is looked at. A page
/// is a position in the project's key order, as in the index.
pub(super) struct Cpp<'a> {
    pages: Vec<(&'a str, &'a File)>,
    /// Per page: the pages its file includes, directly or through another.
    closures: Vec<BTreeSet<usize>>,
    /// The functions by name, a function both trees list once; an
    /// operator's name without spaces (`operator->` of `operator -> ()`).
    by_name: BTreeMap<String, Vec<FnRef>>,
    /// The definitions no file keeps to itself, by key.
    shared: BTreeMap<Key<'a>, Vec<FnRef>>,
    /// Every definition, by page and key.
    own: BTreeMap<(usize, Key<'a>), Vec<FnRef>>,
    /// Every definition, by page, scope and name.
    file_defs: BTreeMap<(usize, &'a str, &'a str), Vec<FnRef>>,
    /// Per scope, name and parameter text with every qualifier dropped
    /// (`fold::unqualified`): each parameter text of a function so keyed.
    spellings: BTreeMap<(&'a str, &'a str, String), BTreeSet<&'a str>>,
    namespaces: BTreeSet<&'a str>,
    /// `namespace_owners`.
    owners: BTreeSet<&'a str>,
    macros: BTreeSet<&'a str>,
    /// `File::specialized`, of every file: each path's scope and name.
    specialized: BTreeSet<(&'a str, &'a str)>,
    /// Type names, the names of the classes the heads define, and owners
    /// of out-of-line definitions that are no namespace or misread.
    types: BTreeSet<&'a str>,
    classes: Classes<'a>,
    /// The stems of the project's unread files (`Project::twins`).
    unread: BTreeSet<&'a str>,
}

impl<'a> Cpp<'a> {
    pub fn new(files: &'a BTreeMap<String, File>, twins: &'a [String]) -> Cpp<'a> {
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
            closures: Vec::new(),
            by_name: BTreeMap::new(),
            shared: BTreeMap::new(),
            own: BTreeMap::new(),
            file_defs: BTreeMap::new(),
            spellings: BTreeMap::new(),
            namespaces: cpp()
                .flat_map(|(_, (_, f))| &f.namespaces)
                .map(String::as_str)
                .collect(),
            owners: namespace_owners(files.values()),
            macros: cpp()
                .flat_map(|(_, (_, f))| &f.macros)
                .map(|m| m.0.as_str())
                .collect(),
            specialized: cpp()
                .flat_map(|(_, (_, f))| &f.specialized)
                .map(|s| s.rsplit_once("::").unwrap_or(("", s)))
                .collect(),
            types: BTreeSet::new(),
            classes: Classes::default(),
            unread: (twins.iter())
                .filter_map(|p| p.rsplit('/').next()?.rsplit_once('.'))
                .map(|s| s.0)
                .collect(),
            pages: pages.clone(),
        };
        // Per page: the pages its includes name.
        let mut includes = vec![Vec::new(); pages.len()];
        for (page, &(path, file)) in cpp() {
            includes[page] = out.includes(&by_file, path, file);
            out.read_items(page, file);
        }
        out.closures = closures(&includes);
        out.read_classes();
        out
    }

    /// The pages the includes of `file`, at `path`, name.
    fn includes(
        &self,
        by_file: &BTreeMap<&str, Vec<usize>>,
        path: &str,
        file: &File,
    ) -> Vec<usize> {
        // `<a/b.h>` may name a project header; `<b.h>` a system one.
        let quoted = file
            .code
            .imports
            .iter()
            .filter_map(|i| match i.strip_prefix('<') {
                Some(a) => a.strip_suffix('>').filter(|a| a.contains('/')),
                None => Some(i.as_str()),
            });
        quoted
            .flat_map(|i| self.include(by_file, path, i))
            .collect()
    }

    /// Reads the items of `file`, at `page`: its types, its functions by
    /// name and its definitions by key.
    fn read_items(&mut self, page: usize, file: &'a File) {
        for (i, it) in file.code.items.iter().enumerate() {
            if it.kind == CodeItemKind::Type {
                self.types.insert(&it.name);
            }
            let owner = it.owner.as_deref().map(owner_key);
            let misread = self.misread((page, i));
            self.types
                .extend(owner.filter(|o| !misread && !self.namespaces.contains(o)));
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
            self.by_name.entry(name).or_default().push(c);
            let key = self.key(c);
            if let Some((scope, name, params)) = key {
                let at = (scope, name, fold::unqualified(params));
                self.spellings.entry(at).or_default().insert(params);
            }
            match key {
                Some(key) if !it.test && !self.declared(c) => {
                    if !file.local.contains(&i) {
                        self.shared.entry(key).or_default().push(c);
                    }
                    self.own.entry((page, key)).or_default().push(c);
                    let named = self.file_defs.entry((page, key.0, key.1));
                    named.or_default().push(c);
                }
                Some(_) | None => {}
            }
        }
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
    pub fn closure(&self, page: usize) -> &BTreeSet<usize> {
        &self.closures[page]
    }

    fn item(&self, c: FnRef) -> &'a CodeItem {
        &self.pages[c.0].1.code.items[c.1]
    }

    fn is_c(&self, page: usize) -> bool {
        // `.C` is C++.
        std::path::Path::new(self.pages[page].0).extension() == Some("c".as_ref())
    }

    /// Is `c` a method no class holds: a name the grammar misread?
    fn misread(&self, c: FnRef) -> bool {
        let file = self.pages[c.0].1;
        file.code.items[c.1].kind == CodeItemKind::Method && !file.classes.contains_key(&c.1)
    }

    /// Is `c` misread and written with a qualifier right before its name
    /// and parameters (`ret_t Kern::f(…)`): a method of a class no fact
    /// names?
    pub(super) fn misread_method(&self, c: FnRef) -> bool {
        let (it, sig) = (self.item(c), self.item(c).signature.as_str());
        let named = format!("::{}", it.name);
        let mut at = sig.match_indices(&named).map(|(i, _)| i + named.len());
        self.misread(c) && at.any(|i| sig[i..].trim_start().starts_with('('))
    }

    /// Is `c` a function template an explicit specialization of the
    /// project names: of its name, in its scope (`fold_scope`) or a path
    /// ending in it?
    fn specialized(&self, c: FnRef) -> bool {
        let (it, scope) = (
            self.item(c),
            fold_scope(self.pages[c.0].1, c.1, &self.owners),
        );
        let at = |path: &str| path == scope || path.ends_with(&format!("::{scope}"));
        let named = self
            .specialized
            .iter()
            .any(|&(path, name)| name == it.name && at(path));
        it.signature.starts_with("template") && named
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
}

/// Per page: the pages it includes, directly or through another, of
/// `includes`, the pages each page's includes name.
fn closures(includes: &[Vec<usize>]) -> Vec<BTreeSet<usize>> {
    let closure = |page: usize| {
        let (mut seen, mut todo) = (BTreeSet::new(), includes[page].clone());
        while let Some(p) = todo.pop() {
            if seen.insert(p) {
                todo.extend(&includes[p]);
            }
        }
        seen
    };
    (0..includes.len()).map(closure).collect()
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
        let via = &call.site.via;
        let found = match via {
            CallVia::Bare => self.cpp_bare(chain, call),
            CallVia::Path(q) => self.cpp_path(chain, call, q),
            CallVia::SelfRecv => match self.member_of(chain) {
                Some(class) => self.on_class(call, class),
                None => self.any_method(call),
            },
            CallVia::Member(x) => self.cpp_member(chain, call, x.as_deref()),
        };
        // A `using std::f;` of the file may bring in another `f`; a
        // function template's explicit specialization, which has no item,
        // may be the one called.
        let foreign = *via == CallVia::Bare && self.foreign_using(&call.site.name);
        let specialized = |f: &FnId| {
            let page = self.index.page_of(&f.path);
            page.is_some_and(|p| self.cpp.specialized((p, f.item)))
        };
        match found {
            Evidence::Found { fns, .. } if foreign || fns.iter().any(specialized) => {
                Evidence::Found { fns, open: true }
            }
            e @ (Evidence::Found { .. } | Evidence::Outside | Evidence::Hidden) => e,
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

    /// The namespace `name` names as a namespace alias of the caller's
    /// file or its includes.
    fn aliased(&self, name: &str) -> Option<&str> {
        let mut pages = std::iter::once(&self.page).chain(self.included);
        let alias = pages.find_map(|&p| self.cpp.pages[p].1.namespace_aliases.get(name));
        alias.map(String::as_str)
    }

    /// The first segment of the namespace a path rooted at `root` names:
    /// through a namespace alias of the caller's file or its includes.
    fn unaliased<'r>(&'r self, root: &'r str) -> &'r str {
        self.aliased(root)
            .map_or(root, |a| a.split("::").next().unwrap_or(""))
    }

    /// Does a using-declaration of the caller's file (at file scope or in
    /// a body: scopes of usings are not modelled) name a function `name`
    /// of a namespace from outside the project: `std`, or one no project
    /// namespace or type roots?
    fn foreign_using(&self, name: &str) -> bool {
        let cpp = self.cpp;
        let usings = self.file.usings.iter();
        let mut declared = usings.filter_map(|u| u.rsplit_once("::").filter(|u| u.1 == name));
        declared.any(|(q, _)| {
            let root = self.unaliased(q.split("::").next().unwrap_or(""));
            root == "std" || !(cpp.namespaces.contains(root) || cpp.types.contains(root))
        })
    }

    /// `q::f()`.
    fn cpp_path(&self, chain: &[FlowScope], call: &MapCall, q: &str) -> Evidence {
        let (cpp, site) = (self.cpp, &call.site);
        // `a::b` of `::a::b::f<T>(…)`.
        let callee = untemplated(call.text.split('(').next().unwrap_or(""));
        let written = callee
            .rsplit_once("::")
            .map_or(q, |w| w.0.trim_start_matches("::"));
        // `Box<int>` of `Box<int>::f()` is the type `Box`; `a` of `::a::f()`.
        let root = match call.root.as_deref() {
            Some("") => written.split("::").next().unwrap_or(""),
            r => r.map_or(q, owner_key),
        };
        // `fs::f()` under `namespace fs = std::filesystem;` (`q` is then
        // `filesystem`) starts at `std`.
        let root = self.unaliased(root);
        let project =
            cpp.types.contains(root) || cpp.namespaces.contains(root) || self.index.is_module(root);
        if (chain.iter()).any(|f| f.rebound.contains(root)) {
            // A template parameter or a local type: any type's method.
            self.any_method(call)
        } else if root == "std" || !project {
            Evidence::Outside
        } else if cpp.types.contains(q) {
            self.on_qualifier(call, q, written)
        } else if cpp.namespaces.contains(q) {
            self.free(site, Some(written)).unwrap_or(Evidence::Hidden)
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
        // A method whose qualified name the grammar misread has no known
        // class: as one no head shows.
        if self.class.is_none() && self.misread && !call.text.starts_with("::") {
            return self.or_free(site, self.made(name, self.any_method(call)));
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
            let or_free = |or| self.or_free(site, or);
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
                Err(Missing::Outside) if !local => {
                    return or_free(self.made(name, Evidence::Outside))
                }
                Err(Missing::Unseen) if !local => {
                    return or_free(self.made(name, self.any_method(call)))
                }
                Err(Missing::Outside | Missing::Unseen | Missing::Nowhere) => {}
            }
        }
        if let Some(found) = self.free(site, None) {
            return found;
        }
        self.made(name, Evidence::Outside)
    }

    /// `or`, a bare call of `name`, unless it reads outside the project:
    /// then any project function of that name misread as a method, or
    /// something outside; with none, not visible when `name` is a project
    /// macro or names a project type, which the call constructs. Not when no class of that name
    /// is in reach and every project alias of it in reach names a type
    /// shown to be from outside: of `std`, or qualified by no project
    /// namespace or type.
    fn made(&self, name: &str, or: Evidence) -> Evidence {
        let outside = match &or {
            Evidence::Outside => true,
            Evidence::Found { fns, .. } => fns.is_empty(),
            Evidence::Hidden => false,
        };
        if !outside {
            return or;
        }
        let cpp = self.cpp;
        // A variable of the file or its includes: a function pointer, an
        // object with a call operator.
        let mut pages = std::iter::once(&self.page).chain(self.included);
        if pages.any(|&p| cpp.pages[p].1.variables.contains(name)) {
            return Evidence::Hidden;
        }
        // A function whose name a macro before its return type misread as
        // a method's: any of those, or something outside.
        let named = cpp.by_name.get(name).into_iter().flatten();
        let misread: Vec<FnRef> = named.copied().filter(|&c| cpp.misread(c)).collect();
        if !misread.is_empty() {
            return Evidence::Found {
                fns: self.ids(misread),
                open: true,
            };
        }
        let aliased = cpp.alias_targets(name, self.sight());
        let outside = |t: &str| match t.split_once("::") {
            Some(("std", _)) => true,
            Some((root, _)) => !cpp.types.contains(root) && !cpp.namespaces.contains(root),
            None => false,
        };
        let class = !cpp.namesakes(name, self.sight()).is_empty();
        let foreign = !class && !aliased.is_empty() && aliased.iter().all(|t| outside(t));
        let project = cpp.macros.contains(name) || cpp.types.contains(name) || !aliased.is_empty();
        if !foreign && project {
            Evidence::Hidden
        } else {
            or
        }
    }

    /// `or`, a bare call of `site` in a method: a member the class may
    /// inherit hides the free functions, so they are the call's, open; one
    /// of a class no head shows (`or` found) may be any method too.
    fn or_free(&self, site: &CallSite, or: Evidence) -> Evidence {
        match (self.free(site, None), or) {
            (Some(Evidence::Found { mut fns, .. }), or) => {
                if let Evidence::Found { fns: more, .. } = or {
                    fns.extend(more);
                }
                Evidence::Found { fns, open: true }
            }
            (Some(_) | None, or) => or,
        }
    }
}
