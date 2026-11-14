//! The candidate index call sites resolve against, and the named candidate
//! sets one caller can draw from. Which set wins is the caller's policy.

use crate::model::{CallSite, CallVia, CodeItem, CodeItemKind};
use crate::support::{module_stem, source_rank};
use crate::CodeUnit;
use std::collections::{BTreeMap, BTreeSet};

/// The last segment of an owner, generics dropped: `Outer.Inner` and
/// `a::Inner<T>` are both `Inner`.
pub fn owner_key(owner: &str) -> &str {
    let head = owner.split('<').next().unwrap_or("");
    head.rsplit([':', '.']).next().unwrap_or("").trim()
}

/// The languages whose code calls each other: a page's code language, with
/// every JavaScript dialect and component format as one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    Rust,
    Python,
    Cpp,
    Go,
    Shell,
    Ecma,
}

impl Family {
    /// The family of a unit's code language; a unit that is not code is `Ecma`.
    pub fn of(lang: Option<&str>) -> Family {
        match lang {
            Some("rust") => Family::Rust,
            Some("python") => Family::Python,
            Some("cpp") => Family::Cpp,
            Some("go") => Family::Go,
            Some("shell") => Family::Shell,
            _ => Family::Ecma,
        }
    }

    /// The family's lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Family::Rust => "rust",
            Family::Python => "python",
            Family::Cpp => "cpp",
            Family::Go => "go",
            Family::Shell => "shell",
            Family::Ecma => "ecma",
        }
    }
}

/// A function item: (page, item), the page a position in the map's key order
/// and the item an index into that unit's items.
pub type FnRef = (usize, usize);

/// What call sites resolve against: every function by name, every page by
/// module name, every type name. A page is a position in the map's key order.
pub struct Index<'a, U: CodeUnit> {
    keys: Vec<&'a str>,
    pages: Vec<&'a U>,
    by_name: BTreeMap<&'a str, Vec<FnRef>>,
    /// Pages by module stem and by directory name.
    modules: BTreeMap<&'a str, BTreeSet<usize>>,
    /// Owner last segments and type item names.
    types: BTreeSet<&'a str>,
    /// A header declaration's definition: its `twin`, else the one function
    /// of the same name and owner in a file of the same stem.
    def_of: BTreeMap<FnRef, FnRef>,
}

impl<'a, U: CodeUnit> Index<'a, U> {
    /// The index over `units`, a page per map entry in key order. Run
    /// `pair_declarations` on `units` first if a header declaration should
    /// fold onto its definition by `twin`; without it a declaration folds only
    /// onto the one same-named function of its owner in a file of its stem.
    pub fn new(units: &'a BTreeMap<String, U>) -> Self {
        let keys: Vec<&str> = units.keys().map(String::as_str).collect();
        let pages: Vec<&U> = units.values().collect();
        let mut by_name: BTreeMap<&str, Vec<FnRef>> = BTreeMap::new();
        let mut by_span: BTreeMap<String, FnRef> = BTreeMap::new();
        let mut modules: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
        let mut types: BTreeSet<&str> = BTreeSet::new();
        for (p, &e) in pages.iter().enumerate() {
            if e.items().is_empty() {
                continue;
            }
            let path = e.path();
            modules.entry(module_stem(path)).or_default().insert(p);
            if let Some(dir) = path.rsplit('/').nth(1) {
                modules.entry(dir).or_default().insert(p);
            }
            for (i, it) in e.items().iter().enumerate() {
                types.extend(it.owner.as_deref().map(owner_key));
                if it.kind == CodeItemKind::Type {
                    types.insert(&it.name);
                }
                // A nested function is no call target: same-named helpers
                // nested in test functions would cross-wire.
                if matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method) && !it.nested {
                    by_name.entry(&it.name).or_default().push((p, i));
                    by_span.insert(it.span(e.path()), (p, i));
                }
            }
        }
        let header = |c: FnRef| source_rank(pages[c.0].path()) < 2;
        let key = |c: FnRef| {
            let it = &pages[c.0].items()[c.1];
            let stem = module_stem(pages[c.0].path());
            (it.owner.as_deref().map(owner_key), stem)
        };
        let mut def_of = BTreeMap::new();
        for fns in by_name.values() {
            for &h in fns.iter().filter(|&&c| header(c)) {
                let twin = pages[h.0].items()[h.1].twin.as_ref();
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
            keys,
            pages,
            by_name,
            modules,
            types,
            def_of,
        }
    }

    /// The item `c` names.
    pub fn item(&self, c: FnRef) -> &'a CodeItem {
        let unit: &'a U = self.pages[c.0];
        &unit.items()[c.1]
    }

    /// The unit of page `page`.
    pub fn unit(&self, page: usize) -> &'a U {
        self.pages[page]
    }

    /// The map key of page `page`.
    pub fn key(&self, page: usize) -> &'a str {
        self.keys[page]
    }

    /// The page of map key `key`.
    pub fn page_of(&self, key: &str) -> Option<usize> {
        self.keys.binary_search(&key).ok()
    }

    /// The family of page `page`'s code language (`Family::of` its `lang`).
    pub fn family(&self, page: usize) -> Family {
        Family::of(self.pages[page].lang())
    }

    /// Is `name` an owner's last segment or a type item's name?
    pub fn is_type(&self, name: &str) -> bool {
        self.types.contains(name)
    }

    /// Is `name` a page's module stem or directory name?
    pub fn is_module(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    /// The candidate sets of a caller on page `page`, owned by `own`, which
    /// sees the pages `sees`.
    pub fn scope<'i>(
        &'i self,
        page: usize,
        own: Option<&'i str>,
        sees: &'i BTreeSet<usize>,
    ) -> Scope<'i, 'a, U> {
        Scope {
            index: self,
            page,
            own,
            sees,
        }
    }

    /// The functions `site` names in the language of page `page` that its
    /// argument count fits and that pass `keep`, each declaration taken as
    /// its definition, deduplicated.
    pub fn pick(&self, page: usize, site: &CallSite, keep: impl Fn(FnRef) -> bool) -> Vec<FnRef> {
        let named = self
            .by_name
            .get(site.name.as_str())
            .map_or(&[][..], Vec::as_slice);
        let lang = self.family(page);
        // `Q::f(x, …)` and Python's `K.f(x, …)` may pass the receiver.
        let path = match &site.via {
            CallVia::Path(_) => true,
            CallVia::Member(Some(x)) => self.types.contains(x.as_str()),
            _ => false,
        };
        let fits = |c: FnRef| match (self.item(c).arity, site.args) {
            (Some(a), Some(n)) => a.fits(n, path),
            _ => true,
        };
        let found: BTreeSet<FnRef> = named
            .iter()
            .copied()
            .filter(|&c| self.family(c.0) == lang && fits(c) && keep(c))
            .map(|c| self.def_of.get(&c).copied().unwrap_or(c))
            .collect();
        found.into_iter().collect()
    }

    fn is_fn(&self, c: FnRef) -> bool {
        self.item(c).kind == CodeItemKind::Fn
    }

    fn is_method(&self, c: FnRef) -> bool {
        self.item(c).kind == CodeItemKind::Method
    }

    fn owned_by(&self, key: &str, c: FnRef) -> bool {
        self.is_method(c) && self.item(c).owner.as_deref().map(owner_key) == Some(key)
    }
}

/// The candidate sets one caller can name: a function owned by `own` on page
/// `page`, which sees the pages `sees`. Each set is `Index::pick` with one
/// filter; none of them cuts a set to a single match.
pub struct Scope<'i, 'a, U: CodeUnit> {
    index: &'i Index<'a, U>,
    page: usize,
    own: Option<&'i str>,
    sees: &'i BTreeSet<usize>,
}

impl<U: CodeUnit> Scope<'_, '_, U> {
    /// Methods of the caller's own owner.
    pub fn own_methods(&self, s: &CallSite) -> Vec<FnRef> {
        let x = self.index;
        self.own
            .map(|o| x.pick(self.page, s, |c| x.owned_by(o, c)))
            .unwrap_or_default()
    }

    /// Free functions in the caller's file.
    pub fn fns_here(&self, s: &CallSite) -> Vec<FnRef> {
        let x = self.index;
        x.pick(self.page, s, |c| x.is_fn(c) && c.0 == self.page)
    }

    /// Free functions in a file the caller sees.
    pub fn fns_seen(&self, s: &CallSite) -> Vec<FnRef> {
        let x = self.index;
        x.pick(self.page, s, |c| x.is_fn(c) && self.sees.contains(&c.0))
    }

    /// Methods in a file the caller sees.
    pub fn methods_seen(&self, s: &CallSite) -> Vec<FnRef> {
        let x = self.index;
        x.pick(self.page, s, |c| x.is_method(c) && self.sees.contains(&c.0))
    }

    /// Free functions of that name anywhere in the project.
    pub fn fns_anywhere(&self, s: &CallSite) -> Vec<FnRef> {
        let x = self.index;
        x.pick(self.page, s, |c| x.is_fn(c))
    }

    /// Methods of that name anywhere in the project.
    pub fn methods_anywhere(&self, s: &CallSite) -> Vec<FnRef> {
        let x = self.index;
        x.pick(self.page, s, |c| x.is_method(c))
    }

    /// `q::f`: methods of an owner whose last segment is `q`, or free
    /// functions in a module (file stem or directory) `q` or in a C++
    /// namespace `q`.
    pub fn qualified(&self, s: &CallSite, q: &str) -> Vec<FnRef> {
        let x = self.index;
        let in_module = x.modules.get(q);
        let in_namespace = |c: FnRef| x.item(c).namespace.as_deref() == Some(q);
        x.pick(self.page, s, |c| {
            x.owned_by(q, c)
                || (x.is_fn(c) && (in_module.is_some_and(|m| m.contains(&c.0)) || in_namespace(c)))
        })
    }
}

/// The pages a caller on page `page` sees: its own, the pages it imports
/// (`imports_of`), and theirs.
pub fn sees(page: usize, imports_of: impl Fn(usize) -> Vec<usize>) -> BTreeSet<usize> {
    let mut sees: BTreeSet<usize> = imports_of(page).into_iter().collect();
    sees.extend(sees.clone().into_iter().flat_map(&imports_of));
    sees.insert(page);
    sees
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CodeFile;
    use std::collections::BTreeMap;

    #[test]
    fn index_scope_and_imports_work_over_code_files() {
        let file = |path: &str, src: &str| (path.to_string(), crate::extract(path, src));
        let units: BTreeMap<String, CodeFile> = [
            file(
                "src/a.rs",
                "use crate::b::helper;\nfn run() { helper(1); }\n",
            ),
            file(
                "src/b.rs",
                "pub fn helper(_x: u8) {}\npub struct S;\nimpl S { pub fn get(&self) {} }\n",
            ),
        ]
        .into();
        let index = Index::new(&units);
        let imports = crate::imports::Imports::new(&units);
        let a = index.page_of("src/a.rs").unwrap();
        assert_eq!(index.family(a), Family::Rust);
        assert_eq!(
            imports.resolve("src/a.rs", "crate::b::helper"),
            ["src/b.rs".to_string()].into()
        );
        assert!(imports
            .resolve("src/a.rs", "std::collections::BTreeMap")
            .is_empty());
        let seen = sees(a, |p| {
            units[index.key(p)]
                .imports
                .iter()
                .flat_map(|i| imports.resolve(index.key(p), i))
                .filter_map(|k| index.page_of(&k))
                .collect()
        });
        assert_eq!(seen.len(), 2);
        let site = &units["src/a.rs"].items[0].call_sites[0];
        let scope = index.scope(a, None, &seen);
        assert_eq!(scope.fns_here(site), Vec::<FnRef>::new());
        let found = scope.fns_seen(site);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (index.key(found[0].0), index.item(found[0]).name.as_str()),
            ("src/b.rs", "helper")
        );
        assert!(index.is_type("S") && index.is_module("b"));
    }
}
