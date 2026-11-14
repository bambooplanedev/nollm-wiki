//! The resolve stage: every call's target in one of four forms, from what
//! the code shows and nothing else.

mod python;

use crate::flow::FlowScope;
use crate::model::{File, FnId, MapCall, Project, Target};
use crate::uses::{defines, in_scope, in_trait, is_trait, own_crate, standard, InScope};
use python::{Own, Python};
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::imports::Imports;
use wiki_code::index::{owner_key, sees, Family, FnRef, Index, Scope};
use wiki_code::model::{CallVia, CodeItem, CodeItemKind};

/// A project whose call targets have been resolved.
pub struct Resolved(Project);

impl Resolved {
    pub fn project(&self) -> &Project {
        &self.0
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
                        scope: index.scope(page, own, &sees),
                        file,
                        item: &file.code.items[item],
                        owner: file.code.items[item].owner.as_deref(),
                        in_trait: in_trait(file, item),
                        family: file.family,
                        names: &in_scope,
                        types: &types,
                        crates: &project.crates,
                    };
                    flow.each_call_mut(&mut |chain, call| {
                        call.target = from_evidence(cx.evidence(chain, call));
                    });
                }
                (path.clone(), flows)
            })
            .collect()
    };
    for (path, flows) in resolved {
        project.files.get_mut(&path).expect("own key").flows = flows;
    }
    Resolved(project)
}

struct Caller<'i, 'a> {
    index: &'i Index<'a, File>,
    page: usize,
    py: &'i Python<'a>,
    scope: Scope<'i, 'a, File>,
    /// The caller's file and item, and its owner as written (`S`, `<S as Tr>`).
    file: &'a File,
    item: &'a CodeItem,
    owner: Option<&'a str>,
    /// The caller is a trait's default method: `self` is any implementor.
    in_trait: bool,
    family: Family,
    names: &'i InScope,
    types: &'i BTreeMap<&'a str, (usize, bool)>,
    crates: &'a BTreeSet<(String, String)>,
}

impl Caller<'_, '_> {
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

    fn evidence(&self, chain: &[FlowScope], call: &MapCall) -> Evidence {
        let (s, site) = (&self.scope, &call.site);
        // The innermost flow that binds the value `x`.
        let binder = |x: &str| chain.iter().rev().find(|f| f.locals.contains(x));
        // A generic parameter, or an item declared in a body.
        let rebound = |x: &str| chain.iter().any(|f| f.rebound.contains(x));
        let python = self.family == Family::Python;
        match &site.via {
            CallVia::Bare
                if binder(&site.name).is_some()
                    || rebound(&site.name)
                    || self.rebinds_alias(chain, call) =>
            {
                Evidence::Hidden
            }
            CallVia::Bare => self.bare(call),
            CallVia::Path(q) if q == "Self" => self.on_self(call, &rebound, false),
            CallVia::Path(q) if q == "self" => self.bare(call),
            // Not this file's own `f` first: any function of that name.
            CallVia::Path(q) if matches!(q.as_str(), "crate" | "super") => Evidence::Found {
                fns: self.ids(s.fns_anywhere(site)),
                open: false,
            },
            CallVia::Path(q) => {
                let root = call.root.as_deref().unwrap_or(q);
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
            CallVia::SelfRecv => self.on_self(call, &rebound, true),
            CallVia::Member(Some(x)) if self.is_cls(x) => self.on_self(call, &rebound, true),
            CallVia::Member(Some(x)) => match binder(x) {
                // A local: its type, when the flow knows one, decides.
                Some(f) => match f.types.get(x) {
                    Some(t) => self.typed(call, t, &rebound),
                    None => self.any_method(call),
                },
                // A Python module from outside the project.
                None if python
                    && self.names.external.contains_key(x)
                    && !self.project_alias(call, x) =>
                {
                    self.external(self.names.external[x], s.qualified(site, x))
                }
                None if rebound(x) || (python && self.py_value(x)) => self.any_method(call),
                None if self.index.is_type(x) || self.index.is_module(x) => {
                    // A Python name no import or class of the file binds.
                    let unbound = !self.names.imported.contains(x) && !defines(self.file, x);
                    let open = !self.sure(x) || (python && unbound);
                    self.found_or_hidden(
                        self.imported_from(x, &site.name, s.qualified(site, x)),
                        open,
                    )
                }
                None => self.any_method(call),
            },
            CallVia::Member(None) if python => self.dotted(chain, call),
            CallVia::Member(None) => self.any_method(call),
        }
    }

    /// `self.f()` (`dot`) and `Self::f()`: a call on the caller's own
    /// type, the `S` of an owner `S` or `<S as Tr>`.
    fn on_self(&self, call: &MapCall, rebound: &dyn Fn(&str) -> bool, dot: bool) -> Evidence {
        let own = self.owner.map_or("", |o| {
            owner_key(
                o.strip_prefix('<')
                    .map_or(o, |o| o.split(" as ").next().unwrap_or("")),
            )
        });
        let found = match self.family {
            // In a trait's default method `self` is any implementor.
            Family::Rust if self.in_trait => return self.any_method(call),
            Family::Rust if own.is_empty() || rebound(own) => Evidence::Hidden,
            Family::Rust => self.of_type(call, own, false),
            Family::Python => return self.py_on_self(call, own, Own::Object),
            // A subclass may override `f`: the subclass-by-text rule comes
            // with these languages' flows. Until then, no certainty.
            Family::Cpp | Family::Ecma | Family::Go | Family::Shell => return Evidence::Hidden,
        };
        match found {
            // Not the type's own method: a trait's, or a deref target's.
            Evidence::Hidden if dot => self.any_method(call),
            Evidence::Hidden | Evidence::Outside | Evidence::Found { .. } => found,
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
                Evidence::Found { mut fns, open } if self.family == Family::Python => {
                    fns.extend(self.ids(self.overrides(name, call)));
                    Evidence::Found { fns, open }
                }
                Evidence::Hidden if self.family == Family::Python => self.any_method(call),
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
        let (cpp, shell) = match self.family {
            Family::Cpp => (true, false),
            Family::Shell => (false, true),
            Family::Rust | Family::Python | Family::Go | Family::Ecma => (false, false),
        };
        if shell && site.name.starts_with('$') {
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
        let none = || Vec::new();
        let own = || if cpp { s.own_methods(site) } else { none() };
        let methods_seen = || if cpp { s.methods_seen(site) } else { none() };
        // A function of another file may be shadowed by an external glob's;
        // a Python name no import names may be a builtin.
        let builtin = self.family == Family::Python && !imported;
        let found = self
            .first(&[&own, &|| s.fns_here(site)], false)
            .or_else(|| {
                // The files a Python import names hold it, or no function.
                if let Some(from) = self.py.sources(self.page, &site.name) {
                    let all = s.fns_anywhere(site).into_iter();
                    let there = || all.clone().filter(|c| from.contains(&c.0)).collect();
                    return self.first(&[&there], !self.sure(&site.name));
                }
                let other: [&dyn Fn() -> Vec<FnRef>; 3] =
                    [&|| s.fns_seen(site), &methods_seen, &|| {
                        s.fns_anywhere(site)
                    }];
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
