//! Declarations folded onto their definitions.

use super::Cpp;
use crate::model::File;
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::{owner_key, Family, FnRef};
use wiki_code::model::CodeItemKind;
use wiki_code::support::source_rank;

/// What a declaration shares with its definition: its scope
/// (`fold_scope`); the name; the parameter types.
pub(super) type Key<'a> = (&'a str, &'a str, &'a str);

/// The names of the C/C++ files of `files`' namespaces and namespace
/// aliases no class head is named: what owns a function as a namespace
/// (`core` of `int core::run(int)`) for `fold_scope`.
pub(crate) fn namespace_owners<'a>(files: impl Iterator<Item = &'a File>) -> BTreeSet<&'a str> {
    let files: Vec<&File> = files.filter(|f| f.family == Family::Cpp).collect();
    let classes: BTreeSet<&str> = (files.iter().flat_map(|f| f.bases.keys()))
        .map(|c| owner_key(c))
        .collect();
    let names = files.iter().flat_map(|f| &f.namespaces);
    names
        .map(String::as_str)
        .filter(|n| !classes.contains(n))
        .collect()
}

/// What may open a fold (`Cpp::folded`).
#[derive(Clone, Copy, Default)]
pub(super) struct Loose {
    /// A definition may be another program's: unless the caller's file
    /// holds one of them (`Cpp::defined_in`).
    program: bool,
    /// A C++ declaration was taken as its file's one definition spelled
    /// otherwise, which may be another overload.
    spelled: bool,
}

impl Loose {
    pub(super) fn or(self, other: Loose) -> Loose {
        Loose {
            program: self.program || other.program,
            spelled: self.spelled || other.spelled,
        }
    }

    /// Is the fold open for a caller whose file holds a definition of it
    /// (`here`)?
    pub(super) fn open(self, here: bool) -> bool {
        (self.program && !here) || self.spelled
    }
}

/// The scope a C/C++ function item `i` of `file` shares with its
/// declarations, `owners` as `namespace_owners` gives: a class method's
/// owner key (`K` of `void n::K::f()` and of `f` declared in `K`); else
/// the namespace path, a qualifier included (`File::scopes`: `a::core` of
/// `int a::core::run(int)` and of a `run` declared in `namespace a {
/// namespace core {…} }`).
pub(crate) fn fold_scope<'f>(file: &'f File, i: usize, owners: &BTreeSet<&str>) -> &'f str {
    let owner = file.code.items[i].owner.as_deref().map(owner_key);
    match owner.filter(|o| !owners.contains(o)) {
        Some(class) => class,
        None => file.scopes.get(&i).map_or("", String::as_str),
    }
}

/// `s` with every qualifier dropped (`vector<Kind>` of
/// `std::vector<core::Kind>`).
pub(super) fn unqualified(s: &str) -> String {
    let mut segs: Vec<&str> = s.split("::").collect();
    let last = segs.pop().unwrap_or("");
    let heads = segs
        .iter()
        .map(|g| g.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_'));
    heads.chain([last]).collect()
}

impl<'a> Cpp<'a> {
    /// `key` with the parameter text of the one other spelling of its scope
    /// and name that reads the same with qualifiers dropped, each name of
    /// one ending the other's (`core::Kind` and `Kind`); none when a third
    /// spelling reads the same too.
    fn requalified(&self, key: Key<'a>) -> Option<Key<'a>> {
        let all = self.spellings.get(&(key.0, key.1, unqualified(key.2)))?;
        let mut other = all.iter().filter(|&&p| p != key.2);
        let (Some(&p), None, 2) = (other.next(), other.next(), all.len()) else {
            return None;
        };
        let names = |s: &'a str| {
            s.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                .filter(|w| !w.is_empty())
        };
        let ends = |a: &str, b: &str| a == b || a.ends_with(&format!("::{b}"));
        let same = names(key.2)
            .zip(names(p))
            .all(|(a, b)| ends(a, b) || ends(b, a));
        same.then_some((key.0, key.1, p))
    }

    pub(super) fn key(&self, c: FnRef) -> Option<Key<'a>> {
        let file = self.pages[c.0].1;
        let params = file.params.get(&c.1)?;
        let scope = fold_scope(file, c.1, &self.owners);
        Some((scope, self.item(c).name.as_str(), params.as_str()))
    }

    /// `fns`, each declaration taken as the definitions of its key: of its
    /// own file when it is local, else those no file keeps to itself; with
    /// none, its file's one definition of that name (and arity, in C++,
    /// where it may be another overload); a declaration with neither
    /// stays. A free function's declarations none
    /// of whose definitions' files includes one (`extern int main(int,
    /// char**);` beside each program's `main`) may be any program's: each
    /// definition of that name and arity, and the set is open. So is it
    /// for a declaration in a header an unread file (`Project::twins`) shares a
    /// stem with (`pad.hpp`, `pad.mm`).
    pub(super) fn folded(&self, fns: Vec<FnRef>) -> (BTreeSet<FnRef>, Loose) {
        let mut out = BTreeSet::new();
        let mut spelled = false;
        // Per free function's key: a declaration, and is one tied?
        let mut tied: BTreeMap<Key, (FnRef, bool)> = BTreeMap::new();
        let mut twin = false;
        for c in fns {
            let Some(key) = self.key(c).filter(|_| self.declared(c)) else {
                out.insert(c);
                continue;
            };
            // An unread file of the header's stem may define it.
            let path = self.pages[c.0].0;
            let stem = path.rsplit('/').next().and_then(|f| f.rsplit_once('.'));
            twin |= source_rank(path) < 2 && stem.is_some_and(|s| self.unread.contains(s.0));
            let page = self.local(c).then_some(c.0);
            let defs = |key| match page {
                Some(p) => self.own.get(&(p, key)),
                None => self.shared.get(&key),
            };
            let defs = defs(key).or_else(|| defs(self.requalified(key)?));
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
                (Some(&d), None) => {
                    spelled |= !self.is_c(c.0);
                    out.insert(d)
                }
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
        let program = twin || !loose.is_empty();
        (out, Loose { program, spelled })
    }
}
