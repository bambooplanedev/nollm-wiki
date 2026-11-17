//! The resolve stage: every call's target in one of four forms, from what
//! the code shows and nothing else.

mod cpp;
mod python;

use crate::flow::FlowScope;
use crate::model::{File, FnId, MapCall, Project, Target};
use crate::uses::{defines, in_scope, in_trait, is_trait, own_crate, standard, InScope};
use cpp::Cpp;
pub(crate) use cpp::{fold_scope, namespace_owners, outer_scope};
use python::Python;
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::imports::Imports;
use wiki_code::index::{owner_key, sees, Family, FnRef, Index, Scope};
use wiki_code::model::{CallVia, CodeItem, CodeItemKind};

/// A project whose call targets have been resolved, with the path of the
/// class a head places each C/C++ method in (`Cpp::class_of`).
pub struct Resolved(Project, BTreeMap<FnId, String>);

impl Resolved {
    pub fn project(&self) -> &Project {
        &self.0
    }

    /// The path of the class a head places the C/C++ method `f` in.
    pub fn class(&self, f: &FnId) -> Option<&str> {
        self.1.get(f).map(String::as_str)
    }
}

/// What the code shows about one call's target.
enum Evidence {
    Hidden,
    Outside,
    /// `open`: the target may also be outside the project.
    Found {
        fns: BTreeSet<FnId>,
        open: bool,
    },
}

impl Evidence {
    /// The candidates of both, which may also be something outside when
    /// either may, or shows no candidate.
    fn and(self, other: Evidence) -> Evidence {
        let (mut fns, mut open) = (BTreeSet::new(), false);
        for e in [self, other] {
            match e {
                Evidence::Found {
                    fns: more,
                    open: also,
                } => {
                    fns.extend(more);
                    open |= also;
                }
                Evidence::Outside | Evidence::Hidden => open = true,
            }
        }
        Evidence::Found { fns, open }
    }

    /// Candidates that may also be something outside when `open`.
    fn open_if(self, open: bool) -> Evidence {
        match self {
            Evidence::Found { fns, open: was } => Evidence::Found {
                fns,
                open: was || open,
            },
            e @ (Evidence::Outside | Evidence::Hidden) => e,
        }
    }
}

/// The one place a `Target` is built. The caller stays among its own
/// candidates: `self.child.depth()` in `depth` may be recursion.
fn from_evidence(e: Evidence) -> Target {
    match e {
        Evidence::Hidden => Target::Hidden,
        Evidence::Outside => Target::Outside,
        Evidence::Found { mut fns, open } => match (fns.len(), open) {
            (0, true) => Target::Outside,
            (0, false) => Target::Hidden,
            (1, false) => Target::Resolved(fns.pop_first().expect("one")),
            _ => Target::OneOf {
                candidates: fns.into_iter().collect(),
                outside: open,
            },
        },
    }
}

pub fn resolve(mut project: Project) -> Resolved {
    let mut classes = BTreeMap::new();
    let resolved: Vec<(String, BTreeMap<usize, crate::model::Flow>)> = {
        let index = Index::new(&project.files);
        let imports = Imports::new(&project.files);
        let pages_of = |p: usize| -> Vec<usize> {
            let key = index.key(p);
            project.files[key]
                .code
                .imports
                .iter()
                .flat_map(|i| imports.resolve(key, i))
                .filter_map(|k| index.page_of(&k))
                .collect()
        };
        // Per type name: how many type items carry it, and is one a trait.
        let mut types: BTreeMap<&str, (usize, bool)> = BTreeMap::new();
        let py = Python::new(&project.files);
        let cpp = Cpp::new(&project.files, &project.twins);
        for f in project.files.values() {
            for it in f
                .code
                .items
                .iter()
                .filter(|it| it.kind == CodeItemKind::Type && !it.test)
            {
                let t = types.entry(&it.name).or_default();
                *t = (t.0 + 1, t.1 || is_trait(it));
            }
        }
        project
            .files
            .iter()
            .enumerate()
            .map(|(page, (path, file))| {
                for &item in file.cpp.classes.keys() {
                    if let Some(class) = cpp.placed((page, item)) {
                        let f = FnId {
                            path: path.clone(),
                            item,
                        };
                        classes.insert(f, class.to_string());
                    }
                }
                let sees = sees(page, pages_of);
                let in_scope = in_scope(path, file, &imports, &index, &py.roots, &project.crates);
                let mut flows = file.flows.clone();
                for (&item, flow) in &mut flows {
                    // An owner like `<W as Display>` has no key: no own methods.
                    let own = file.code.items[item]
                        .owner
                        .as_deref()
                        .map(owner_key)
                        .filter(|k| !k.is_empty());
                    let cx = Caller {
                        index: &index,
                        page,
                        py: &py,
                        cpp: &cpp,
                        scope: index.scope(page, own, &sees),
                        file,
                        item: &file.code.items[item],
                        in_trait: in_trait(file, item),
                        names: &in_scope,
                        types: &types,
                        crates: &project.crates,
                    };
                    let (rust, python, c);
                    let rules: &dyn Rules = match file.family {
                        Family::Cpp => {
                            c = cpp::Caller::new(&cx, item);
                            &c
                        }
                        Family::Python => {
                            python = python::Caller(&cx);
                            &python
                        }
                        Family::Rust => {
                            rust = Rust(&cx);
                            &rust
                        }
                        // No flow is made for these: no call to look at.
                        Family::Go | Family::Shell | Family::Ecma => continue,
                    };
                    flow.each_call_mut(&mut |chain, call| {
                        call.target = from_evidence(rules.evidence(chain, call));
                    });
                }
                (path.clone(), flows)
            })
            .collect()
    };
    for (path, flows) in resolved {
        project.files.get_mut(&path).expect("own key").flows = flows;
    }
    Resolved(project, classes)
}

/// One language family's call rules.
trait Rules {
    /// What the code shows about the target of `call`, `chain` the flows
    /// enclosing it, outermost first.
    fn evidence(&self, chain: &[FlowScope], call: &MapCall) -> Evidence;
}

/// What every family's rules read about the function a call is in.
struct Caller<'i, 'a> {
    index: &'i Index<'a, File>,
    page: usize,
    py: &'i Python<'a>,
    cpp: &'i Cpp<'a>,
    scope: Scope<'i, 'a, File>,
    /// The caller's file and item; the item's owner is as written (`S`, `<S as Tr>`).
    file: &'a File,
    item: &'a CodeItem,
    /// The caller is a trait's default method: `self` is any implementor.
    in_trait: bool,
    names: &'i InScope,
    types: &'i BTreeMap<&'a str, (usize, bool)>,
    crates: &'a BTreeSet<(String, String)>,
}

/// The innermost flow of `chain` that binds the value `x`.
fn binder<'c>(chain: &'c [FlowScope], x: &str) -> Option<&'c FlowScope> {
    chain.iter().rev().find(|f| f.locals.contains(x))
}

/// Is `x` a generic parameter, or an item declared in a body, in `chain`?
fn rebound(chain: &[FlowScope], x: &str) -> bool {
    chain.iter().any(|f| f.rebound.contains(x))
}

/// Rust's call rules.
struct Rust<'c, 'i, 'a>(&'c Caller<'i, 'a>);

impl Rules for Rust<'_, '_, '_> {
    fn evidence(&self, chain: &[FlowScope], call: &MapCall) -> Evidence {
        let (cx, site) = (self.0, &call.site);
        let rebound = |x: &str| rebound(chain, x);
        match &site.via {
            CallVia::Bare if binder(chain, &site.name).is_some() || rebound(&site.name) => {
                Evidence::Hidden
            }
            CallVia::Bare => cx.bare(call),
            CallVia::Path(q) if q == "Self" => self.on_self(call, &rebound, false),
            CallVia::Path(q) => cx.path(call, q, &rebound),
            CallVia::SelfRecv => self.on_self(call, &rebound, true),
            CallVia::Member(Some(x)) => match binder(chain, x) {
                Some(f) => cx.on_local(call, f, x, &rebound),
                None if rebound(x) => cx.any_method(call),
                None if cx.index.is_type(x) || cx.index.is_module(x) => {
                    let fns = cx.imported_from(x, &site.name, cx.scope.qualified(site, x));
                    cx.found_or_hidden(fns, !cx.sure(x))
                }
                None => cx.any_method(call),
            },
            CallVia::Member(None) => cx.any_method(call),
        }
    }
}

impl Rust<'_, '_, '_> {
    /// `self.f()` (`dot`) and `Self::f()`: a call on the caller's own
    /// type (`Caller::own`).
    fn on_self(&self, call: &MapCall, rebound: &dyn Fn(&str) -> bool, dot: bool) -> Evidence {
        let (cx, own) = (self.0, self.0.own());
        // In a trait's default method `self` is any implementor.
        if cx.in_trait {
            return cx.any_method(call);
        }
        let found = if own.is_empty() || rebound(own) {
            Evidence::Hidden
        } else {
            cx.of_type(call, own, false)
        };
        match found {
            // Not the type's own method: a trait's, or a deref target's.
            Evidence::Hidden if dot => cx.any_method(call),
            Evidence::Hidden | Evidence::Outside | Evidence::Found { .. } => found,
        }
    }
}

impl<'a> Caller<'_, 'a> {
    fn ids(&self, fns: impl IntoIterator<Item = FnRef>) -> BTreeSet<FnId> {
        fns.into_iter()
            // Test code is not compiled into the code that calls it.
            .filter(|&c| !self.index.item(c).test)
            .map(|(p, item)| FnId {
                path: self.index.key(p).to_string(),
                item,
            })
            .collect()
    }

    /// The first set that is not empty, as evidence open or not.
    fn first(&self, sets: &[&dyn Fn() -> Vec<FnRef>], open: bool) -> Option<Evidence> {
        sets.iter()
            .map(|s| self.ids(s()))
            .find_map(|fns| (!fns.is_empty()).then_some(Evidence::Found { fns, open }))
    }

    fn found_or_hidden(&self, fns: Vec<FnRef>, open: bool) -> Evidence {
        let fns = self.ids(fns);
        if fns.is_empty() {
            Evidence::Hidden
        } else {
            Evidence::Found { fns, open }
        }
    }

    /// Can `name` be only the project's? Not when an external glob may
    /// bring it in and the file neither defines nor names it, nor when an
    /// outside alias of that original name is folded into it.
    fn sure(&self, name: &str) -> bool {
        (!self.names.glob || self.names.imported.contains(name) || defines(self.file, name))
            && !self.names.contested.contains(name)
    }

    /// Any project method of the call's name, or something outside.
    fn any_method(&self, call: &MapCall) -> Evidence {
        Evidence::Found {
            fns: self.ids(self.scope.methods_anywhere(&call.site)),
            open: true,
        }
    }

    /// A name from outside the project. From the standard crates it is
    /// outside. Another crate may be the project's own library (`use
    /// mycrate::run;` in `main.rs`): the project's `fns` of that name, or
    /// something outside.
    fn external(&self, std: bool, fns: Vec<FnRef>) -> Evidence {
        let fns = self.ids(fns);
        if std || fns.is_empty() {
            Evidence::Outside
        } else {
            Evidence::Found { fns, open: true }
        }
    }

    /// What `q::f()` or a call on a value of type `q` reaches: the
    /// methods of the project type `q` (or the functions of a module
    /// `q`). Several types of that name, or a trait: any method named `f`.
    fn of_type(&self, call: &MapCall, q: &str, open: bool) -> Evidence {
        let (s, site) = (&self.scope, &call.site);
        // A Python import says which of the types named `q` is meant.
        let named = self.py.sources(self.page, q).is_some();
        match self.types.get(q) {
            Some(&(n, is_trait)) if (n > 1 && !named) || is_trait => {
                self.found_or_hidden(s.methods_anywhere(site), true)
            }
            _ => {
                let fns = self.imported_from(q, &site.name, s.qualified(site, q));
                self.found_or_hidden(fns, open)
            }
        }
    }

    /// The caller's own type: the `S` of an owner `S` or `<S as Tr>`.
    fn own(&self) -> &'a str {
        self.item.owner.as_deref().map_or("", |o| {
            owner_key(
                o.strip_prefix('<')
                    .map_or(o, |o| o.split(" as ").next().unwrap_or("")),
            )
        })
    }

    /// `x.f()` on the local `x` the flow `f` binds: its type, when the
    /// flow knows one, decides.
    fn on_local(
        &self,
        call: &MapCall,
        f: &FlowScope,
        x: &str,
        rebound: &dyn Fn(&str) -> bool,
    ) -> Evidence {
        match f.types.get(x) {
            Some(t) => self.typed(call, t, rebound),
            None => self.any_method(call),
        }
    }

    /// `q::f()`, `q` not `Self`.
    fn path(&self, call: &MapCall, q: &str, rebound: &dyn Fn(&str) -> bool) -> Evidence {
        let (s, site) = (&self.scope, &call.site);
        if q == "self" {
            return self.bare(call);
        }
        // Not this file's own `f` first: any function of that name.
        if matches!(q, "crate" | "super") {
            return Evidence::Found {
                fns: self.ids(s.fns_anywhere(site)),
                open: false,
            };
        }
        let root = call
            .path
            .as_deref()
            .map_or(q, |p| p.split("::").next().unwrap_or("").trim());
        if standard(self.file, root) {
            Evidence::Outside
        } else if let Some(&std) = self.names.external.get(root) {
            self.external(std, s.qualified(site, q))
        } else if rebound(root) || self.renamed(root) {
            // A generic parameter: any type the caller is given.
            self.any_method(call)
        // A module the file declares or imports shadows the crate.
        } else if own_crate(self.file, root, self.crates)
            && !defines(self.file, root)
            && !self.names.local(root)
        {
            // `mycrate::f()` is `crate::f()` of that crate.
            let found = if root == q {
                Evidence::Found {
                    fns: self.ids(s.fns_anywhere(site)),
                    open: false,
                }
            } else {
                self.of_type(call, q, false)
            };
            self.in_crate(root, found)
        // An alias of a project module or type: `q` is what it names.
        } else if self.project_root(root, root == q) || self.names.aliases.contains(root) {
            self.of_type(call, q, root == q && !self.sure(q))
        } else {
            Evidence::Hidden
        }
    }

    /// Is `name` a `use … as` alias that a project type is also called?
    /// Then it is not that type.
    fn renamed(&self, name: &str) -> bool {
        self.names.aliases.contains(name) && self.index.is_type(name)
    }

    /// Is `c` a method a trait declares or a trait impl defines?
    fn by_trait(&self, c: FnRef) -> bool {
        let owner = self.index.item(c).owner.as_deref();
        owner.is_some_and(|o| o.starts_with('<')) || in_trait(self.index.unit(c.0), c.1)
    }

    /// `x.f()` on `x: t`, `t` the type as written (`Vec<u8>`,
    /// `std::rc::Rc<S>`). A project type: its methods. An external type
    /// with no project type among its generic arguments: outside, unless a
    /// project trait has a method of that name or the type may be the
    /// project library's. Any other type (a generic parameter, a prelude
    /// wrapper, an alias of a project type's name): any method.
    fn typed(&self, call: &MapCall, t: &str, rebound: &dyn Fn(&str) -> bool) -> Evidence {
        let (head, args) = t.split_once('<').unwrap_or((t, ""));
        let name = owner_key(head);
        let root = head.split("::").next().unwrap_or("").trim();
        let std = standard(self.file, root);
        let foreign = self.names.external.get(root) == Some(&false);
        let project_arg = args
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|w| !w.is_empty() && (self.index.is_type(w) || rebound(w)));
        let project = !head.contains("::") || self.project_root(root, false);
        if rebound(root) || self.renamed(root) {
            self.any_method(call)
        } else if (std || self.names.external.contains_key(root)) && !project_arg {
            let all = self.scope.methods_anywhere(&call.site).into_iter();
            let mut fns: Vec<FnRef> = all.filter(|&c| self.by_trait(c)).collect();
            if foreign {
                fns.extend(self.scope.qualified(&call.site, name));
            }
            self.external(false, fns)
        } else if !std && !foreign && project && self.index.is_type(name) {
            let found = self.of_type(call, name, !head.contains("::") && !self.sure(name));
            match found {
                // A value of a Python class may be a subclass's, and
                // may call a method it inherits.
                Evidence::Found { mut fns, open } if self.file.family == Family::Python => {
                    fns.extend(self.ids(self.overrides(name, call)));
                    Evidence::Found { fns, open }
                }
                Evidence::Hidden if self.file.family == Family::Python => self.any_method(call),
                Evidence::Hidden | Evidence::Outside | Evidence::Found { .. } => found,
            }
        } else {
            self.any_method(call)
        }
    }

    /// Can a path starting at `root` name project code? `crate`, `self`,
    /// `super`, a project module or type, one of the project's own crates,
    /// or any `root` that is the whole qualifier (`single`: `Q::f`, which
    /// `qualified` then checks). Another crate's path (`serde_json::Value`)
    /// cannot.
    fn project_root(&self, root: &str, single: bool) -> bool {
        matches!(root, "crate" | "self" | "super")
            || self.index.is_module(root)
            || own_crate(self.file, root, self.crates)
            || (single || self.index.is_type(root))
    }

    /// Is the file at `path` in the crate `name`? Its crate is the one
    /// whose directory is the longest prefix of its path.
    fn in_crate_of(&self, path: &str, name: &str) -> bool {
        let dirs = self.crates.iter().map(|(_, d)| d.as_str());
        let holding = dirs.filter(|d| d.is_empty() || path.starts_with(&format!("{d}/")));
        holding
            .max_by_key(|d| d.len())
            .is_some_and(|d| self.crates.contains(&(name.to_string(), d.to_string())))
    }

    /// The functions of `found` in the crate `name`. None of them: the
    /// crate may re-export one, or something outside.
    fn in_crate(&self, name: &str, found: Evidence) -> Evidence {
        let Evidence::Found { fns, open } = found else {
            return found;
        };
        let mine: BTreeSet<FnId> = fns
            .iter()
            .filter(|f| self.in_crate_of(&f.path, name))
            .cloned()
            .collect();
        match (fns.is_empty(), mine.is_empty()) {
            (false, true) => Evidence::Found { fns, open: true },
            (true, _) | (false, false) => Evidence::Found { fns: mine, open },
        }
    }

    fn bare(&self, call: &MapCall) -> Evidence {
        let (s, site) = (&self.scope, &call.site);
        if self.file.family == Family::Shell && site.name.starts_with('$') {
            return Evidence::Hidden;
        }
        let external = self.names.external.get(&site.name).copied();
        let imported = self.names.imported.contains(&site.name);
        if imported && !s.fns_here(site).is_empty() {
            // Defined here and imported: a `use` in some body or module
            // may or may not reach this call.
            return Evidence::Found {
                fns: self.ids(s.fns_anywhere(site)),
                open: external.is_some(),
            };
        }
        if let Some(std) = external {
            return self.external(std, s.fns_anywhere(site));
        }
        // A function of another file may be shadowed by an external glob's;
        // a Python name no import names may be a builtin.
        let builtin = self.file.family == Family::Python && !imported;
        let found = self.first(&[&|| s.fns_here(site)], false).or_else(|| {
            // The files a Python import names hold it, or no function.
            if let Some(from) = self.py.sources(self.page, &site.name) {
                let all = s.fns_anywhere(site).into_iter();
                let there = || all.clone().filter(|c| from.contains(&c.0)).collect();
                return self.first(&[&there], !self.sure(&site.name));
            }
            let other: [&dyn Fn() -> Vec<FnRef>; 2] =
                [&|| s.fns_seen(site), &|| s.fns_anywhere(site)];
            self.first(&other, builtin || !self.sure(&site.name))
        });
        // A name imported from one own crate is that crate's.
        let krate = self
            .names
            .from_crate
            .get(&site.name)
            .filter(|k| k.len() == 1);
        match (found, krate.and_then(|k| k.first())) {
            (Some(found), Some(k)) => self.in_crate(k, found),
            (Some(found), None) => found,
            // A constructor of a project type (`Meters(5)`), or a name a
            // project import brings in (`use Shape::Circle;`): no function
            // the map shows, and no outside call.
            (None, _)
                if imported || self.index.is_type(&site.name) || defines(self.file, &site.name) =>
            {
                Evidence::Hidden
            }
            (None, _) => Evidence::Outside,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(item: usize) -> FnId {
        FnId {
            path: "src/a.rs".to_string(),
            item,
        }
    }

    fn found(items: &[usize], open: bool) -> Evidence {
        Evidence::Found {
            fns: items.iter().map(|&i| id(i)).collect(),
            open,
        }
    }

    fn one_of(items: &[usize], outside: bool) -> Target {
        Target::OneOf {
            candidates: items.iter().map(|&i| id(i)).collect(),
            outside,
        }
    }

    #[test]
    fn from_evidence_is_certain_only_when_closed_and_single() {
        assert_eq!(from_evidence(Evidence::Hidden), Target::Hidden);
        assert_eq!(from_evidence(Evidence::Outside), Target::Outside);
        assert_eq!(from_evidence(found(&[1], false)), Target::Resolved(id(1)));
        assert_eq!(from_evidence(found(&[1], true)), one_of(&[1], true));
        assert_eq!(from_evidence(found(&[2, 1], false)), one_of(&[1, 2], false));
        assert_eq!(from_evidence(found(&[], false)), Target::Hidden);
        assert_eq!(from_evidence(found(&[], true)), Target::Outside);
    }
}
