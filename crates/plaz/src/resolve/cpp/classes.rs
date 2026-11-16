//! The classes of the project's C/C++ files, each by its path
//! (`File::classes`): what its bases are, which classes derive from it,
//! and what name lookup finds from it.

use super::{fits, Cpp};
use crate::model::File;
use crate::source::untemplated;
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::{owner_key, Family, FnRef};
use wiki_code::model::{CallSite, CodeItemKind};

/// What a base a class head names is.
#[derive(Clone, Copy)]
enum Base<'a> {
    /// The project class of this path.
    Class(&'a str),
    /// No project type.
    Outside,
    /// A project type no head places: an alias, or a name several
    /// classes share.
    Unseen,
}

/// Why no class of a lineage defines a name, the least known last.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Missing {
    /// Every class on the way is a project class whose head a file shows.
    Nowhere,
    /// A base is no project type: the name may be its member.
    Outside,
    /// A class no head places: it may derive from anything.
    Unseen,
}

#[derive(Default)]
pub(super) struct Classes<'a> {
    /// Per class whose head a file shows: its bases.
    bases: BTreeMap<&'a str, Vec<Base<'a>>>,
    /// Per class: the project classes naming it as a base.
    derived: BTreeMap<&'a str, BTreeSet<&'a str>>,
    /// Per base name as written, by `owner_key`: the classes naming it.
    naming: BTreeMap<&'a str, BTreeSet<&'a str>>,
    /// Per class name (`owner_key`): the paths of the classes it names.
    by_key: BTreeMap<&'a str, BTreeSet<&'a str>>,
}

impl<'a> Cpp<'a> {
    /// Fills `classes` from every C/C++ file's heads, once the project's
    /// types and namespaces are known.
    pub(super) fn read_classes(&mut self) {
        let files = self.pages.iter().map(|p| p.1);
        let files: Vec<&'a File> = files.filter(|f| f.family == Family::Cpp).collect();
        let heads = || files.iter().flat_map(|f| &f.bases);
        for (class, _) in heads() {
            self.types.insert(owner_key(class));
            let paths = self.classes.by_key.entry(owner_key(class)).or_default();
            paths.insert(class);
            self.classes.bases.entry(class).or_default();
        }
        let named = heads().flat_map(|(c, bases)| bases.iter().map(move |b| (c.as_str(), b)));
        let read: Vec<(&str, &str, Base)> = named
            .map(|(class, b)| (class, b.as_str(), self.base(b, class)))
            .collect();
        for (class, written, base) in read {
            if let Base::Class(p) = base {
                self.classes.derived.entry(p).or_default().insert(class);
            }
            let naming = self.classes.naming.entry(owner_key(written));
            naming.or_default().insert(class);
            self.classes.bases.entry(class).or_default().push(base);
        }
    }

    fn base(&self, written: &str, from: &str) -> Base<'a> {
        match self.resolve(written, from) {
            Some(p) => Base::Class(p),
            None if self.is_type(written) => Base::Unseen,
            None => Base::Outside,
        }
    }

    /// The project class the type written `t` names from the scope `from`
    /// (a path): `from::t`, else that of each enclosing scope, else the
    /// one class whose path ends in `t`.
    pub(super) fn resolve(&self, t: &str, from: &str) -> Option<&'a str> {
        let t = untemplated(t);
        let t = t.trim_start_matches("::");
        if t.starts_with("std::") {
            return None;
        }
        let heads = self.classes.by_key.get(owner_key(t))?;
        let mut scope = from;
        loop {
            let want = match scope {
                "" => t.to_string(),
                s => format!("{s}::{t}"),
            };
            if let Some(&p) = heads.get(want.as_str()) {
                return Some(p);
            }
            if scope.is_empty() {
                break;
            }
            scope = scope.rsplit_once("::").map_or("", |s| s.0);
        }
        match self.namesakes(t)[..] {
            [p] => Some(p),
            _ => None,
        }
    }

    /// The classes whose path is the type written `t` or ends in it.
    pub(super) fn namesakes(&self, t: &str) -> Vec<&'a str> {
        let t = untemplated(t);
        let t = t.trim_start_matches("::");
        let heads = self.classes.by_key.get(owner_key(t)).into_iter().flatten();
        let named = heads.filter(|p| **p == t || p.ends_with(&format!("::{t}")));
        named.copied().collect()
    }

    /// Is the type written `t` a project type's name, under no qualifier
    /// but a project namespace or type?
    pub(super) fn is_type(&self, t: &str) -> bool {
        let t = untemplated(t);
        let t = t.trim_start_matches("::");
        let root = t.split("::").next().unwrap_or("");
        let rooted =
            !t.contains("::") || self.namespaces.contains(root) || self.types.contains(root);
        root != "std" && rooted && self.types.contains(owner_key(t))
    }

    /// The class of the method item `i` of `page` (`File::classes`):
    /// none for a namespace's function; a path written short is the one
    /// class whose path ends in it.
    pub fn class_of(&self, page: usize, i: usize) -> Option<&'a str> {
        let written = self.pages[page].1.classes.get(&i)?.as_str();
        if let Some((&class, _)) = self.classes.bases.get_key_value(written) {
            return Some(class);
        }
        let namespace = self.namespaces.contains(owner_key(written));
        (!namespace).then(|| self.resolve(written, "").unwrap_or(written))
    }

    /// The methods `site` names that `class` defines, by argument count,
    /// folded.
    pub(super) fn members(&self, class: &str, site: &CallSite) -> BTreeSet<FnRef> {
        let named = self.by_name.get(site.name.as_str()).into_iter().flatten();
        let own = named.copied().filter(|&c| {
            let it = self.item(c);
            let of = self.pages[c.0].1.classes.get(&c.1);
            it.kind == CodeItemKind::Method && of.is_some_and(|o| o == class) && fits(it, site)
        });
        self.folded(own.collect())
    }

    /// The project classes deriving from `class`, directly or not.
    fn descendants(&self, class: &'a str) -> BTreeSet<&'a str> {
        let mut out = BTreeSet::new();
        let mut todo = vec![class];
        while let Some(c) = todo.pop() {
            for &sub in self.classes.derived.get(c).into_iter().flatten() {
                if sub != class && out.insert(sub) {
                    todo.push(sub);
                }
            }
        }
        out
    }

    /// The methods `site` names of the project classes whose heads name
    /// the type written `t` as a base, and of theirs.
    pub(super) fn deriving(&self, t: &str, site: &CallSite) -> BTreeSet<FnRef> {
        let named = self.classes.naming.get(owner_key(t)).into_iter().flatten();
        let all = named.flat_map(|&c| self.descendants(c).into_iter().chain([c]));
        all.flat_map(|c| self.members(c, site)).collect()
    }

    /// What a call of `site`'s name on an object of `class` may reach: the
    /// methods name lookup finds in the class or its nearest project bases,
    /// with each project subclass's own; a pure virtual declaration only
    /// while no subclass defines the name; and whether it may also be
    /// outside (only pure virtual declarations found).
    pub(super) fn lookup(
        &self,
        class: &'a str,
        site: &CallSite,
    ) -> Result<(BTreeSet<FnRef>, bool), Missing> {
        let mut seen = BTreeSet::from([class]);
        let (mut level, mut missing) = (vec![class], Missing::Nowhere);
        while !level.is_empty() {
            let mut found: BTreeSet<FnRef> =
                level.iter().flat_map(|c| self.members(c, site)).collect();
            if !found.is_empty() {
                let subs = self.descendants(class).into_iter();
                let overrides: BTreeSet<FnRef> = subs.flat_map(|c| self.members(c, site)).collect();
                let open = overrides.is_empty() && found.iter().all(|&c| self.pure(c));
                if !overrides.is_empty() {
                    found.retain(|&c| !self.pure(c));
                }
                found.extend(overrides);
                return Ok((found, open));
            }
            let mut next = Vec::new();
            for c in level {
                let Some(bases) = self.classes.bases.get(c) else {
                    missing = Missing::Unseen;
                    continue;
                };
                for &b in bases {
                    match b {
                        Base::Class(p) if seen.insert(p) => next.push(p),
                        Base::Class(_) => {}
                        Base::Outside => missing = missing.max(Missing::Outside),
                        Base::Unseen => missing = Missing::Unseen,
                    }
                }
            }
            level = next;
        }
        Err(missing)
    }

    /// Is `c` a pure virtual declaration (`virtual int f() = 0`)?
    fn pure(&self, c: FnRef) -> bool {
        let sig = self.item(c).signature.trim_end();
        let zero = sig.strip_suffix('0');
        self.declared(c) && zero.is_some_and(|s| s.trim_end().ends_with('='))
    }
}
