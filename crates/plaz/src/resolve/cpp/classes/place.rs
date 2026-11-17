//! Placing a written type name: C++ name lookup over the project's
//! classes and aliases.

use super::{bare, Base, Placed, Sight};
use crate::resolve::cpp::Cpp;
use crate::source::untemplated;
use std::collections::BTreeSet;
use wiki_code::index::owner_key;
use wiki_code::support::source_rank;

/// One lookup's result, over one set of classes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step<'a> {
    Found(&'a str),
    Unplaced,
    Nothing,
}

impl<'a> Cpp<'a> {
    /// Where the type written `t`, seen from the scope `from` (a path) in
    /// the file of `sight`, lands. The one entry for placing a written
    /// name: a base, a typed receiver, a qualifier, an out-of-line
    /// method's class.
    ///
    /// C++ lookup order: at each scope from `from` outward a project alias
    /// or class of that path (the file's `using` directives at the global
    /// scope), the class itself by its own name, then the aliases and
    /// classes nested in its nearest project bases (a base outside the
    /// project hides none, nor does one whose template arguments name a
    /// template parameter; one no head places stops the lookup); last, the
    /// one class whose path ends in `t`, unless `t` starts with `::`, which
    /// names the global namespace only. Run twice: over the classes an
    /// includer may provide too (every header's, outside its anonymous
    /// namespaces) besides those in files `sight` sees, and over those in
    /// files `sight` sees only; a class at one path counts once. Both find
    /// it, or only the first does: that class. They differ: either. An
    /// alias, a base no head places, or several classes met on the way:
    /// unplaced.
    pub(in crate::resolve::cpp) fn place_name(
        &self,
        t: &str,
        from: &str,
        sight: Sight,
    ) -> Placed<'a> {
        let t = untemplated(t);
        // `::K` is looked up from the global namespace only.
        let rooted = t.starts_with("::");
        let (t, from) = match t.strip_prefix("::") {
            Some(t) => (t, ""),
            None => (t.as_str(), from),
        };
        if t.starts_with("std::") {
            return Placed::Nothing;
        }
        let seen = |page: usize, _: &str| sight.sees(page);
        let reachable = |page: usize, path: &str| self.reaches(sight, page, path);
        let place = |usings: &[&str]| {
            let all = self.lookup_name(t, from, sight, &reachable, usings, rooted);
            let mine = self.lookup_name(t, from, sight, &seen, usings, rooted);
            match (all, mine) {
                (Step::Unplaced, _) | (_, Step::Unplaced) => Placed::Unplaced,
                (Step::Found(a), Step::Found(m)) if a != m => Placed::Either(a, m),
                (Step::Found(a), _) => Placed::Class(a),
                (Step::Nothing, Step::Found(m)) => Placed::Class(m),
                (Step::Nothing, Step::Nothing) => Placed::Nothing,
            }
        };
        // Usings in a body or a named namespace may or may not reach
        // `from`: place with them and without.
        let file = self.pages[sight.page].1;
        let outer: Vec<&str> = file.cpp.file_usings.iter().map(String::as_str).collect();
        let inner = file.cpp.inner_usings.iter();
        let inner = inner.filter(|u| !file.cpp.file_usings.contains(*u));
        let all: Vec<&str> = outer
            .iter()
            .copied()
            .chain(inner.map(String::as_str))
            .collect();
        let without = place(&outer);
        if all.len() == outer.len() {
            return without;
        }
        match (without, place(&all)) {
            (one, other) if one == other => one,
            (Placed::Class(one), Placed::Class(other)) => Placed::Either(one, other),
            _ => Placed::Unplaced,
        }
    }

    /// Is the class or alias at `path` in `page` in reach of the file of
    /// `sight`: in a file it sees, or, as an includer may provide it, in a
    /// header outside its anonymous namespaces?
    fn reaches(&self, sight: Sight, page: usize, path: &str) -> bool {
        let header = source_rank(self.pages[page].0) < 2;
        sight.sees(page) || (header && !path.contains("(anonymous@"))
    }

    /// The types the project aliases named `name` in reach of the file of
    /// `sight` name (`CppFacts::aliases`).
    pub(in crate::resolve::cpp) fn alias_targets(&self, name: &str, sight: Sight) -> Vec<&'a str> {
        let all = self.classes.aliases.get(name).into_iter().flatten();
        let reached = all.filter(|a| self.reaches(sight, a.0, a.1));
        reached
            .filter_map(|a| self.pages[a.0].1.cpp.aliases.get(a.1).map(String::as_str))
            .collect()
    }

    /// `place_name`'s lookup over the classes and aliases `keep` lets in
    /// (by a page holding one and its path).
    fn lookup_name(
        &self,
        t: &str,
        from: &str,
        sight: Sight,
        keep: &dyn Fn(usize, &str) -> bool,
        usings: &[&str],
        rooted: bool,
    ) -> Step<'a> {
        let key = owner_key(t);
        let heads = self.classes.by_key.get(key).into_iter().flatten();
        let kept = |p: &&&'a str| {
            let at = self
                .classes
                .heads
                .get(**p)
                .into_iter()
                .flat_map(|h| h.keys());
            at.into_iter().any(|&pg| keep(pg, p))
        };
        let cands: Vec<(&'a str, String)> = heads.filter(kept).map(|&p| (p, bare(p))).collect();
        let aliases = self.classes.aliases.get(key).into_iter().flatten();
        let aliases: Vec<String> = (aliases.filter(|a| keep(a.0, a.1)))
            .map(|a| bare(a.1))
            .collect();
        // What the paths `wants` name: a class, nothing, or unplaced.
        let at = |wants: &[String]| {
            if aliases.iter().any(|a| wants.contains(a)) {
                return Step::Unplaced;
            }
            let mut hit = cands.iter().filter(|c| wants.contains(&c.1)).map(|c| c.0);
            match (hit.next(), hit.next()) {
                (None, _) => Step::Nothing,
                (Some(p), None) => Step::Found(p),
                (Some(_), Some(_)) => Step::Unplaced,
            }
        };
        let mut scope = from;
        loop {
            let mut wants = vec![match bare(scope).as_str() {
                "" => t.to_string(),
                s => format!("{s}::{t}"),
            }];
            if scope.is_empty() {
                // `using n::t;` names that one entity, before any other:
                // with no project class, something outside, unless `n` is
                // rooted in a project namespace or namespace alias.
                let declared = usings.iter().filter(|u| u.rsplit("::").next() == Some(t));
                let declared: Vec<String> = declared.map(ToString::to_string).collect();
                let project =
                    |u: &String| self.namespaces.contains(u.split("::").next().unwrap_or(""));
                if !declared.is_empty() {
                    return match at(&declared) {
                        Step::Nothing if declared.iter().any(project) => Step::Unplaced,
                        step @ (Step::Found(_) | Step::Unplaced | Step::Nothing) => step,
                    };
                }
                // `using namespace n;` gives `n::t`.
                wants.extend(usings.iter().map(|u| format!("{u}::{t}")));
            }
            match at(&wants) {
                Step::Nothing => {}
                found @ (Step::Found(_) | Step::Unplaced) => return found,
            }
            // A class's own name in its body is the class.
            let own = self.classes.heads.get_key_value(scope).map(|h| *h.0);
            if let Some(p) = own.filter(|_| owner_key(scope) == t) {
                return Step::Found(p);
            }
            match self.in_bases(scope, &|b| at(&[format!("{}::{t}", bare(b))]), sight) {
                Step::Nothing => {}
                found @ (Step::Found(_) | Step::Unplaced) => return found,
            }
            if scope.is_empty() {
                break;
            }
            scope = scope.rsplit_once("::").map_or("", |s| s.0);
        }
        // `::K` is no class whose path only ends in `K`.
        if rooted {
            return Step::Nothing;
        }
        let ends = format!("::{t}");
        if aliases.iter().any(|a| a == t || a.ends_with(&ends)) {
            return Step::Unplaced;
        }
        let mut named = cands.iter().filter(|c| c.1 == t || c.1.ends_with(&ends));
        match (named.next(), named.next()) {
            (None, _) => Step::Nothing,
            (Some(c), None) => Step::Found(c.0),
            (Some(_), Some(_)) => Step::Unplaced,
        }
    }

    /// What `nested(base)` gives for the nearest project bases of the
    /// class `scope` (the bases of the head `sight` sees) that give
    /// something: unplaced past a base no head places or when one level
    /// gives several.
    fn in_bases(&self, scope: &str, nested: &dyn Fn(&str) -> Step<'a>, sight: Sight) -> Step<'a> {
        let mut level = vec![scope];
        let mut seen = BTreeSet::from([scope]);
        while !level.is_empty() {
            let mut next = Vec::new();
            let bases = level.iter().filter_map(|c| self.bases_of(c, sight));
            for b in bases.flatten() {
                match b {
                    Base::Class(p, false) if seen.insert(p) => next.push(p),
                    // An outside base hides no project type name; a
                    // dependent one is not searched for a name written bare.
                    Base::Class(..) | Base::Outside => {}
                    Base::Unseen => return Step::Unplaced,
                }
            }
            let found: BTreeSet<Step> = next.iter().map(|b| nested(b)).collect();
            let found: Vec<Step> = found.into_iter().filter(|s| *s != Step::Nothing).collect();
            match found[..] {
                [] => level = next,
                [one] => return one,
                _ => return Step::Unplaced,
            }
        }
        Step::Nothing
    }
}
