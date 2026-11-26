//! C and C++'s own call rules: the files the includes reach (quoted
//! ones, and angle ones naming a directory),
//! functions local to their file, declarations folded onto their
//! definitions, and the classes a method call may dispatch to.

mod classes;
mod fold;
mod free;
mod members;

use super::{Evidence, Rules};
use crate::flow::FlowScope;
use crate::model::{Bodiless, File, FnId, MapCall};
use crate::source::{included, project_include, untemplated};
use classes::{Classes, Missing, Sight};
use fold::{fold_scope, namespace_owners, outer_scope};
use fold::{Key, Loose};
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::{owner_key, Family, FnRef};
use wiki_code::model::{CallSite, CallVia, CodeItem, CodeItemKind};
use wiki_code::support::source_rank;

/// What the project's C/C++ files say before any call is looked at. A page
/// is a position in the project's key order, as in the index.
pub(super) struct Cpp<'a> {
    pages: Vec<(&'a str, &'a File)>,
    /// Per page: the pages its file includes, directly or through another.
    closures: Vec<BTreeSet<usize>>,
    /// The pages some file includes, directly or through another.
    included: BTreeSet<usize>,
    /// The pages with a quoted include that names no project file: a
    /// generated header, or one found through a build include path.
    blind: BTreeSet<usize>,
    /// Per page: `closures`, with what an angle include of a project
    /// file's name (`<kit.h>`, found through `-I`) may bring in too.
    macro_reach: Vec<BTreeSet<usize>>,
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
    /// Per name of a namespace, a class, an alias or a method's written
    /// class: each path one of that name stands directly under, anonymous
    /// namespaces dropped; empty for the global namespace.
    under: BTreeMap<&'a str, BTreeSet<String>>,
    /// `namespace_owners`.
    owners: BTreeSet<&'a str>,
    macros: BTreeSet<&'a str>,
    /// `CppFacts::specialized`, of every file: each path's scope and name.
    specialized: BTreeSet<(&'a str, &'a str)>,
    /// Type names, the names of the classes the heads define, and owners
    /// of out-of-line definitions that are no namespace or misread.
    types: BTreeSet<&'a str>,
    /// Per type name: how many type items the C/C++ files declare by it.
    type_items: BTreeMap<&'a str, usize>,
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
            included: BTreeSet::new(),
            blind: BTreeSet::new(),
            macro_reach: Vec::new(),
            by_name: BTreeMap::new(),
            shared: BTreeMap::new(),
            own: BTreeMap::new(),
            file_defs: BTreeMap::new(),
            spellings: BTreeMap::new(),
            namespaces: cpp()
                .flat_map(|(_, (_, f))| &f.cpp.namespaces)
                .map(String::as_str)
                .collect(),
            under: BTreeMap::new(),
            owners: namespace_owners(files.values()),
            macros: cpp()
                .flat_map(|(_, (_, f))| &f.cpp.macros)
                .map(|m| m.0.as_str())
                .collect(),
            specialized: cpp()
                .flat_map(|(_, (_, f))| &f.cpp.specialized)
                .map(|s| s.rsplit_once("::").unwrap_or(("", s)))
                .collect(),
            types: BTreeSet::new(),
            type_items: BTreeMap::new(),
            classes: Classes::default(),
            unread: (twins.iter())
                .filter_map(|p| p.rsplit('/').next()?.rsplit_once('.'))
                .map(|s| s.0)
                .collect(),
            pages: pages.clone(),
        };
        // Per page: the pages its includes name.
        let mut includes = vec![Vec::new(); pages.len()];
        let mut reach = vec![Vec::new(); pages.len()];
        for (page, &(path, file)) in cpp() {
            let (blind, angled);
            (includes[page], angled, blind) = out.includes(&by_file, path, file);
            reach[page] = includes[page].iter().chain(&angled).copied().collect();
            if blind {
                out.blind.insert(page);
            }
            out.read_items(page, file);
        }
        out.closures = closures(&includes);
        out.macro_reach = closures(&reach);
        out.included = out.closures.iter().flatten().copied().collect();
        out.read_classes();
        out
    }

    /// The pages the includes of `file`, at `path`, name; those its angle
    /// includes the system's would be, `<b.h>`, name; and whether a quoted
    /// one names none.
    fn includes(
        &self,
        by_file: &BTreeMap<&str, Vec<usize>>,
        path: &str,
        file: &File,
    ) -> (Vec<usize>, Vec<usize>, bool) {
        let (mut pages, mut angled, mut blind) = (Vec::new(), Vec::new(), false);
        for import in &file.code.imports {
            let project = project_include(import);
            let system = import.strip_prefix('<').and_then(|i| i.strip_suffix('>'));
            let Some(inc) = project.or(system) else {
                continue;
            };
            let named = by_file.get(inc.rsplit('/').next().unwrap_or(inc));
            let named = named.into_iter().flatten().map(|&p| (p, self.pages[p].0));
            let found = included(&named.collect::<Vec<_>>(), path, inc);
            blind |= found.is_empty() && !import.starts_with('<');
            match project {
                Some(_) => pages.extend(found),
                None => angled.extend(found),
            }
        }
        (pages, angled, blind)
    }

    /// Reads the items of `file`, at `page`: its types, its functions by
    /// name and its definitions by key.
    fn read_items(&mut self, page: usize, file: &'a File) {
        let paths = (file.cpp.namespace_paths.iter())
            .chain(file.cpp.bases.keys())
            .chain(file.cpp.aliases.keys())
            .chain(file.cpp.classes.values());
        for path in paths {
            let mut above: Vec<&str> = Vec::new();
            for seg in path.split("::").filter(|s| !s.starts_with("(anonymous@")) {
                let at = self.under.entry(seg).or_default();
                at.insert(above.join("::"));
                above.push(seg);
            }
        }
        for (i, it) in file.code.items.iter().enumerate() {
            if it.kind == CodeItemKind::Type {
                self.types.insert(&it.name);
                *self.type_items.entry(&it.name).or_default() += 1;
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
                    if !file.cpp.local.contains(&i) {
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

    /// The pages the file of `page` includes, directly or through another.
    pub fn closure(&self, page: usize) -> &BTreeSet<usize> {
        &self.closures[page]
    }

    /// May an includer provide what the file of `page` names: is it a
    /// header, or a file another includes?
    fn shared(&self, page: usize) -> bool {
        source_rank(self.pages[page].0) < 2 || self.included.contains(&page)
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
        file.code.items[c.1].kind == CodeItemKind::Method && !file.cpp.classes.contains_key(&c.1)
    }

    /// Is `c` misread and written out of its class (`out_of_line`): a
    /// method of a class no fact names?
    pub(super) fn misread_method(&self, c: FnRef) -> bool {
        self.misread(c) && self.out_of_line(c)
    }

    /// Is `c` written with a qualifier right before its name and
    /// parameters (`ret_t Kern::f(…)`)?
    fn out_of_line(&self, c: FnRef) -> bool {
        let (it, sig) = (self.item(c), self.item(c).signature.as_str());
        let named = format!("::{}", it.name);
        let mut at = sig.match_indices(&named).map(|(i, _)| i + named.len());
        at.any(|i| sig[i..].trim_start().starts_with('('))
    }

    /// Is `c` a function template an explicit specialization of the
    /// project names: of its name, in its scope (`fold_scope`,
    /// `outer_scope`) or a path ending in it?
    fn specialized(&self, c: FnRef) -> bool {
        let (it, file) = (self.item(c), self.pages[c.0].1);
        let scope = fold_scope(file, c.1, &self.owners);
        let ends = |path: &str, s: &str| path == s || path.ends_with(&format!("::{s}"));
        // One written in an inline namespace is found by its parent's path.
        let at = |path: &str| {
            ends(path, scope) || outer_scope(file, c.1, scope).is_some_and(|s| ends(path, s))
        };
        let named = self
            .specialized
            .iter()
            .any(|&(path, name)| name == it.name && at(path));
        it.signature.starts_with("template") && named
    }

    fn local(&self, c: FnRef) -> bool {
        self.pages[c.0].1.cpp.local.contains(&c.1)
    }

    /// Does a definition among `fns` stand in the file of `page`? A call
    /// there reaches its own unit's definition, whatever `folded` opened.
    fn defined_in(&self, fns: &BTreeSet<FnRef>, page: usize) -> bool {
        fns.iter().any(|&c| c.0 == page && !self.declared(c))
    }

    fn declared(&self, c: FnRef) -> bool {
        self.pages[c.0].1.cpp.declarations.get(&c.1) == Some(&Bodiless::Declared)
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

/// C and C++'s call rules, with what only they read about the caller.
pub(super) struct Caller<'c, 'i, 'a> {
    cx: &'c super::Caller<'i, 'a>,
    /// The pages the caller's file includes, directly or not.
    included: &'i BTreeSet<usize>,
    /// The caller's namespace path, its qualifier included (`CppFacts::scopes`).
    ns: &'a str,
    /// A method's own class (`Cpp::class_of`).
    class: Option<&'a str>,
    /// A method whose qualified name the grammar misread
    /// (`Cpp::misread_method`): no class is known.
    misread: bool,
}

impl<'i, 'a> std::ops::Deref for Caller<'_, 'i, 'a> {
    type Target = super::Caller<'i, 'a>;

    fn deref(&self) -> &Self::Target {
        self.cx
    }
}

impl<'c, 'i, 'a> Caller<'c, 'i, 'a> {
    /// The rules for the calls of `cx`'s item, the `item`th of its file.
    pub(super) fn new(cx: &'c super::Caller<'i, 'a>, item: usize) -> Self {
        let (cpp, c) = (cx.cpp, (cx.page, item));
        Caller {
            cx,
            included: cpp.closure(c.0),
            ns: cx.file.cpp.scopes.get(&c.1).map_or("", String::as_str),
            class: cpp.class_of(c.0, c.1),
            misread: cpp.misread_method(c),
        }
    }
}

impl Rules for Caller<'_, '_, '_> {
    fn evidence(&self, chain: &[FlowScope], call: &MapCall) -> Evidence {
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
        // function template's explicit specialization, which is no
        // candidate, may be the one called.
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
}

impl Caller<'_, '_, '_> {
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
        let alias = pages.find_map(|&p| self.cpp.pages[p].1.cpp.namespace_aliases.get(name));
        alias.map(String::as_str)
    }

    /// The first segment of the namespace a path rooted at `root` names:
    /// through a namespace alias of the caller's file or its includes.
    fn unaliased<'r>(&'r self, root: &'r str) -> &'r str {
        self.aliased(root)
            .map_or(root, |a| a.split("::").next().unwrap_or(""))
    }

    /// Can a qualifier written `::name` name something of the project: a
    /// namespace, class or alias `name` of the global namespace, or one a
    /// file-scope `using` of the caller's file or its includes brings
    /// there (`using namespace n;`, `using n::name;`)?
    fn at_root(&self, name: &str) -> bool {
        let pages = std::iter::once(&self.page).chain(self.included);
        let usings: Vec<&String> = pages
            .flat_map(|&p| &self.cpp.pages[p].1.cpp.file_usings)
            .collect();
        let used = |path: &str| {
            let named = |u: &&String| path == u.as_str() || path.ends_with(&format!("::{u}"));
            usings.iter().any(named)
        };
        let above = self.cpp.under.get(name).into_iter().flatten();
        above
            .into_iter()
            .any(|a| a.is_empty() || used(a) || used(&format!("{a}::{name}")))
    }

    /// Does a using-declaration of the caller's file (at file scope or in
    /// a body: scopes of usings are not modelled) name a function `name`
    /// of a namespace from outside the project: `std`, or one no project
    /// namespace or type roots?
    fn foreign_using(&self, name: &str) -> bool {
        let cpp = self.cpp;
        let usings = self.file.cpp.usings.iter();
        let mut declared = usings.filter_map(|u| u.rsplit_once("::").filter(|u| u.1 == name));
        declared.any(|(q, _)| {
            let root = self.unaliased(q.split("::").next().unwrap_or(""));
            root == "std" || !(cpp.namespaces.contains(root) || cpp.types.contains(root))
        })
    }

    /// `q::f()`.
    fn cpp_path(&self, chain: &[FlowScope], call: &MapCall, q: &str) -> Evidence {
        let (cpp, site) = (self.cpp, &call.site);
        // `::a::b` of `::a::b::f<T>(…)`: a leading `::` names the global
        // namespace only, wherever the qualifier is looked up.
        let callee = untemplated(call.callee.as_deref().unwrap_or(""));
        let written = callee.rsplit_once("::").map_or(q, |w| w.0);
        // `Box` of `Box<int>::f()`; `a` of `::a::f()`.
        let root = written.trim_start_matches("::").split("::").next();
        let root = root.unwrap_or(q);
        // `fs::f()` under `namespace fs = std::filesystem;` (`q` is then
        // `filesystem`) starts at `std`.
        // `::a` the project has only under another namespace is not it;
        // in a header, or a file another includes, a `using` of the
        // includer may still bring it out.
        let absent = written.starts_with("::")
            && !cpp.shared(self.page)
            && self.aliased(root).is_none()
            && !self.at_root(root);
        let root = self.unaliased(root);
        let project = !absent
            && (cpp.types.contains(root)
                || cpp.namespaces.contains(root)
                || self.index.is_module(root));
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
        // `::f()` names no local or member.
        let global = call.text.starts_with("::");
        // A local called is its own call operator.
        let local = chain.iter().rev().find(|f| f.locals.contains(name));
        if let Some(f) = local.filter(|_| !global) {
            return match f.types.get(name) {
                Some(t) => {
                    let local = self.called(call, t, self.class.unwrap_or(self.ns), self.sight());
                    self.or_field(chain, f, call, name, None, local)
                }
                None => Evidence::Hidden,
            };
        }
        if (chain.iter()).any(|f| f.rebound.contains(name)) {
            return Evidence::Hidden;
        }
        // A method whose qualified name the grammar misread has no known
        // class: as one no head shows.
        if self.class.is_none() && self.misread && !global {
            return self.or_free(site, self.made(name, self.any_method(call)));
        }
        if let Some(class) = self.class.filter(|_| !global) {
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
                    let members = Evidence::Found {
                        fns: self.ids(found.fns),
                        open: false,
                    };
                    let free = self.free(site, None).unwrap_or(Evidence::Outside);
                    return members.and(free, local);
                }
                Ok(found) => return self.found(found),
                Err(Missing::Outside) if !local => {
                    return or_free(self.made(name, Evidence::Outside))
                }
                Err(Missing::Unseen) if !local => {
                    return or_free(self.made(name, self.any_method(call)))
                }
                Err(Missing::Outside | Missing::Unseen | Missing::Nowhere) => {}
            }
        }
        if let Some(found) = self.free(site, global.then_some("::")) {
            return found;
        }
        self.made(name, Evidence::Outside)
    }

    /// `or`, a bare call of `name`, unless it reads outside the project:
    /// then any project function of that name misread as a method, or
    /// something outside; with none, not visible when `name` is a macro
    /// the caller's file sees or names a project type, which the call
    /// constructs. Not when no class of that name
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
        if pages.any(|&p| cpp.pages[p].1.cpp.variables.contains(name)) {
            return Evidence::Hidden;
        }
        // A macro of the file or its includes, an angle include of a
        // project file's name among them, or of a quoted include the walk
        // cannot match; in a header, or a file another includes, one of the
        // includer's too.
        let mut pages = std::iter::once(&self.page).chain(&cpp.macro_reach[self.page]);
        let defines = |p: usize| {
            cpp.blind.contains(&p) || cpp.pages[p].1.cpp.macros.iter().any(|m| m.0 == name)
        };
        let macro_seen =
            cpp.macros.contains(name) && (cpp.shared(self.page) || pages.any(|&p| defines(p)));
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
        let project = macro_seen || cpp.types.contains(name) || !aliased.is_empty();
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
            (Some(free @ Evidence::Found { .. }), or) => free.and(or, true),
            (Some(_) | None, or) => or,
        }
    }

    /// What a lookup found, as evidence.
    fn found(&self, found: classes::Found) -> Evidence {
        Evidence::Found {
            fns: self.ids(found.fns),
            open: found.open,
        }
    }
}
