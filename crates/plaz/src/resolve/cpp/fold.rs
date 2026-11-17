//! Declarations folded onto their definitions.

use super::Cpp;
use crate::model::{Bodiless, File};
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
    let classes: BTreeSet<&str> = (files.iter().flat_map(|f| f.cpp.bases.keys()))
        .map(|c| owner_key(c))
        .collect();
    let names = files.iter().flat_map(|f| &f.cpp.namespaces);
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
/// the namespace path, a qualifier included, inline namespaces too
/// (`CppFacts::inline_scopes`, else `CppFacts::scopes`: `a::core` of `int
/// a::core::run(int)` and of a `run` declared in `namespace a { namespace
/// core {…} }`).
pub(crate) fn fold_scope<'f>(file: &'f File, i: usize, owners: &BTreeSet<&str>) -> &'f str {
    let owner = file.code.items[i].owner.as_deref().map(owner_key);
    match owner.filter(|o| !owners.contains(o)) {
        Some(class) => class,
        None => {
            (file.cpp.inline_scopes.get(&i).or(file.cpp.scopes.get(&i))).map_or("", String::as_str)
        }
    }
}

/// The scope a function item `i` of `file` whose `fold_scope` is `scope`
/// is also named by: with `scope` its path through inline namespaces,
/// that path without them (`a` of `f` in `a::v2`, `v2` inline).
pub(crate) fn outer_scope<'f>(file: &'f File, i: usize, scope: &str) -> Option<&'f str> {
    file.cpp.inline_scopes.get(&i).filter(|s| *s == scope)?;
    Some(file.cpp.scopes.get(&i).map_or("", String::as_str))
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
    /// `key`, of the declaration `c`, with the parameter text of the one
    /// other spelling of its scope and name that reads the same with
    /// qualifiers dropped, each name of one ending the other's (`core::Kind`
    /// and `Kind`) where the function writing the shorter shows it to be
    /// the longer (`means`); none when a third spelling reads the same
    /// too, or when a name the two spell apart is that of two project
    /// types (`a::Kind` and `b::Kind`, or a nested `Box::Kind`).
    fn requalified(&self, c: FnRef, key: Key<'a>) -> Option<Key<'a>> {
        let all = self.spellings.get(&(key.0, key.1, unqualified(key.2)))?;
        let mut other = all.iter().filter(|&&p| p != key.2);
        let (Some(&p), None, 2) = (other.next(), other.next(), all.len()) else {
            return None;
        };
        // The functions spelled the other way.
        let others = self.by_name.get(key.1).into_iter().flatten();
        let others: Vec<FnRef> = others
            .copied()
            .filter(|&d| self.key(d) == Some((key.0, key.1, p)))
            .collect();
        let names = |s: &'a str| {
            s.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                .filter(|w| !w.is_empty())
        };
        let ends = |a: &str, b: &str| a == b || a.ends_with(&format!("::{b}"));
        // A type item, or a class head, nested ones included.
        let one = |a: &str| {
            let (items, heads) = (
                self.type_items.get(owner_key(a)),
                self.classes.by_key.get(owner_key(a)),
            );
            items.is_none_or(|&n| n < 2) && heads.is_none_or(|h| h.len() < 2)
        };
        // The function writing the short name shows it to be the long one.
        let shown = |a: &str, b: &str| {
            (ends(a, b) && others.iter().all(|&d| self.means(d, b, a, c)))
                || (ends(b, a) && others.iter().all(|&d| self.means(c, a, b, d)))
        };
        let same = (names(key.2).zip(names(p))).all(|(a, b)| a == b || (shown(a, b) && one(a)));
        same.then_some((key.0, key.1, p))
    }

    /// Does the function `x` show that `short`, a name in its parameters,
    /// is `long`, which ends in it and is written by `y` (`Kind` and
    /// `core::Kind`): `x` sits in the namespace or class `long` adds
    /// (`core`, read through a namespace alias of `y`'s file or includes),
    /// or a file-scope `using` of `x`'s file or includes names that
    /// namespace or `long`, and no `using` there names another `short` (`using
    /// std::string;` beside `my::string`); or `long` adds only the name
    /// of the class `short` starts with (`Box::Box::Lid` is `Box::Lid`);
    /// or the nearest alias `short` in a namespace around `x` names `long`.
    /// Never when `short` is a template parameter in scope at `x`
    /// (`CppFacts::template_params`).
    fn means(&self, x: FnRef, short: &str, long: &str, y: FnRef) -> bool {
        let facts = &self.pages[x.0].1.cpp;
        let Some(added) = long.strip_suffix(short).and_then(|p| p.strip_suffix("::")) else {
            return false;
        };
        // `short` is a template parameter in scope at `x`: no type around it.
        if (facts.template_params.get(&x.1)).is_some_and(|p| p.contains(short)) {
            return false;
        }
        if short.split("::").next() == Some(added) && !self.namespaces.contains(added) {
            return true;
        }
        let (root, rest) = added.split_once("::").unwrap_or((added, ""));
        let mut theirs = std::iter::once(&y.0).chain(self.closure(y.0));
        let alias = theirs.find_map(|&p| self.pages[p].1.cpp.namespace_aliases.get(root));
        let added = match alias {
            Some(a) if rest.is_empty() => a.clone(),
            Some(a) => format!("{a}::{rest}"),
            None => added.to_string(),
        };
        let scopes = [&facts.classes, &facts.inline_scopes, &facts.scopes];
        let at = scopes.iter().find_map(|s| s.get(&x.1));
        let at = at.map_or("", String::as_str);
        let sits = format!("::{at}::").contains(&format!("::{added}::"));
        let pages = || std::iter::once(&x.0).chain(self.closure(x.0));
        let usings: Vec<&String> = pages().flat_map(|&p| &self.pages[p].1.cpp.usings).collect();
        let either = |a: &str, b: &str| {
            a == b || a.ends_with(&format!("::{b}")) || b.ends_with(&format!("::{a}"))
        };
        // The nearest alias `short` around `x` names `long`.
        let around = std::iter::successors(Some(at), |s| s.rsplit_once("::").map(|s| s.0));
        let mut aliased = around.filter_map(|s| {
            let path = format!("{s}::{short}");
            pages().find_map(|&p| self.pages[p].1.cpp.aliases.get(&path))
        });
        if aliased
            .next()
            .is_some_and(|t| *t == format!("{added}::{short}"))
        {
            return true;
        }
        let name = short.rsplit("::").next();
        let another = |u: &&String| u.rsplit_once("::").map(|u| u.1) == name && !either(u, long);
        // Only a `using` at file scope is known to reach `x`.
        let mut reaching = pages().flat_map(|&p| &self.pages[p].1.cpp.file_usings);
        !usings.iter().any(another)
            && (sits || reaching.any(|u| either(u, long) || either(u, &added)))
    }

    /// `key`, of the declaration `c` in an inline namespace, in the
    /// namespace around it, where its definition may be written (`int
    /// a::f()` for `f` of `a::v2`, `v2` inline); none when another
    /// declaration is that one's too: one written there, or one of
    /// another inline namespace.
    fn outer(&self, c: FnRef, key: Key<'a>) -> Option<Key<'a>> {
        let at = |d: FnRef, key: Key<'a>| {
            Some((outer_scope(self.pages[d.0].1, d.1, key.0)?, key.1, key.2))
        };
        let outer = at(c, key)?;
        let named = self.by_name.get(key.1).into_iter().flatten();
        let mut others = named.filter(|&&d| self.declared(d) && self.key(d) != Some(key));
        let rival = others.any(|&d| {
            let other = self.key(d);
            other == Some(outer) || other.and_then(|k| at(d, k)) == Some(outer)
        });
        (!rival).then_some(outer)
    }

    /// Per declaration of the project, its definitions with a section:
    /// those a call of it is folded onto (`folded`); with none, those no
    /// declaration is folded onto (`unfolded`), but not the one such
    /// definition another overload's declaration wants too. Never one of
    /// another class or specialization (`same_class`). Those of the
    /// declaration's parameters, qualifiers dropped, in a file that
    /// includes the declaration's or is included by it win, when any is;
    /// else an include may keep several of the rest, never one.
    pub(in crate::resolve) fn definitions(&self) -> BTreeMap<FnRef, Vec<FnRef>> {
        let declared = (self.pages.iter().enumerate()).flat_map(|(page, (_, f))| {
            let all = f.cpp.declarations.iter();
            all.filter(|d| *d.1 == Bodiless::Declared)
                .map(move |d| (page, *d.0))
        });
        let mut out: BTreeMap<FnRef, Vec<FnRef>> = declared
            .map(|c| {
                let defs = self.folded(vec![c]).0.into_iter();
                let defs = defs.filter(|&d| d != c && self.sectioned(d) && self.same_class(c, d));
                (c, defs.collect())
            })
            .collect();
        let claimed: BTreeSet<FnRef> = out.values().flatten().copied().collect();
        // Per definition no fold reaches: the keys of the declarations
        // that may be spelled otherwise.
        let mut wanted: BTreeMap<FnRef, BTreeSet<Option<Key>>> = BTreeMap::new();
        for (&c, defs) in out.iter_mut().filter(|o| o.1.is_empty()) {
            defs.extend(self.unfolded(c, &claimed));
            for &d in defs.iter() {
                wanted.entry(d).or_default().insert(self.key(c));
            }
        }
        let params = |d: FnRef| self.key(d).map(|k| unqualified(k.2));
        for (&c, defs) in &mut out {
            // Either file includes the other: a platform twin's is not it.
            let tied = |d: &FnRef| {
                d.0 == c.0 || self.closure(d.0).contains(&c.0) || self.closure(c.0).contains(&d.0)
            };
            // The overloads of the declaration's parameters, qualifiers
            // dropped, that an include ties win over the rest.
            let same: Vec<FnRef> = (defs.iter().copied())
                .filter(|&d| params(c).is_some() && params(d) == params(c))
                .collect();
            let tied_same: Vec<FnRef> = same.iter().copied().filter(tied).collect();
            if !tied_same.is_empty() {
                *defs = tied_same;
            } else if defs.iter().filter(|d| tied(d)).count() > 1 {
                // Of overloads matched by arity alone, an include never
                // picks one.
                defs.retain(tied);
            }
            // The one definition two overloads want by arity may be the
            // other's.
            if let [d] = defs[..] {
                if !same.contains(&d) && wanted.get(&d).is_some_and(|k| k.len() > 1) {
                    defs.clear();
                }
            }
        }
        out
    }

    /// The definitions with a section of the declaration `c`'s scope, name
    /// and arity, of its class, that are not `claimed`: a type spelled
    /// otherwise (`fs::path` and `std::filesystem::path`) keeps them from a
    /// fold.
    fn unfolded(&self, c: FnRef, claimed: &BTreeSet<FnRef>) -> Vec<FnRef> {
        let (it, owners) = (self.item(c), &self.owners);
        let scope = |d: FnRef| fold_scope(self.pages[d.0].1, d.1, owners);
        let named = self.by_name.get(&it.name).into_iter().flatten().copied();
        named
            .filter(|&d| {
                !self.declared(d)
                    && self.sectioned(d)
                    && !claimed.contains(&d)
                    && (d.0 == c.0 || !self.local(d))
                    && scope(d) == scope(c)
                    && self.item(d).arity == it.arity
                    && self.same_class(c, d)
            })
            .collect()
    }

    /// Is `d` a definition with a section: charted, or defaulted out of
    /// its class? A deleted function and one defaulted in its class are
    /// their own first declarations.
    fn sectioned(&self, d: FnRef) -> bool {
        let f = self.pages[d.0].1;
        let defaulted = f.cpp.declarations.get(&d.1) == Some(&Bodiless::Defaulted);
        f.flows.contains_key(&d.1) || (defaulted && self.out_of_line(d))
    }

    /// May `c` and `d` be methods of one class: placed in the same, else
    /// one path (`class_of`) ending the other, and their owners the same
    /// specialization when both are written with template arguments?
    fn same_class(&self, c: FnRef, d: FnRef) -> bool {
        let ends = |a: &str, b: &str| a == b || a.ends_with(&format!("::{b}"));
        let class = |x: FnRef| self.class_of(x.0, x.1);
        let owner = |x: FnRef| {
            let o = self.item(x).owner.as_deref().unwrap_or("");
            o.contains('<').then(|| o.replace(char::is_whitespace, ""))
        };
        let same = match (self.placed(c), self.placed(d)) {
            (Some(a), Some(b)) => a == b,
            _ => class(c)
                .zip(class(d))
                .is_none_or(|(a, b)| ends(a, b) || ends(b, a)),
        };
        same && owner(c).zip(owner(d)).is_none_or(|(a, b)| a == b)
    }

    pub(super) fn key(&self, c: FnRef) -> Option<Key<'a>> {
        let file = self.pages[c.0].1;
        let params = file.cpp.params.get(&c.1)?;
        let scope = fold_scope(file, c.1, &self.owners);
        Some((scope, self.item(c).name.as_str(), params.as_str()))
    }

    /// `fns`, each declaration taken as the definitions of its key: of its
    /// own file when it is local, else those no file keeps to itself; with
    /// none, its file's one definition of that name (and arity, in C++,
    /// where it may be another overload); a declaration with neither
    /// stays. A free function's declarations none
    /// of whose definitions' files includes one or is included by its file
    /// (`extern int main(int, char**);` beside each program's `main`) may
    /// be any program's: each
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
            // One declared in an inline namespace may be defined in its
            // parent, whatever else is defined at its own path.
            let outer = self.outer(c, key).and_then(defs);
            let named = defs(key).or_else(|| defs(self.requalified(c, key)?));
            if named.is_some() || outer.is_some() {
                let defs: Vec<FnRef> = (named.into_iter().chain(outer))
                    .flatten()
                    .copied()
                    .collect();
                out.extend(&defs);
                if page.is_none() && self.item(c).kind == CodeItemKind::Fn {
                    // Either file includes the other (a header its `.inl`).
                    let includes = |d: &FnRef| {
                        d.0 == c.0
                            || self.closure(d.0).contains(&c.0)
                            || self.closure(c.0).contains(&d.0)
                    };
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
