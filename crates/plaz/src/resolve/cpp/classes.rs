//! The classes of the project's C/C++ files, each by its path
//! (`File::classes`): what its bases are, which classes derive from it,
//! and what name lookup finds from it.

use super::{fits, Cpp};
use crate::source::untemplated;
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::{owner_key, Family, FnRef};
use wiki_code::model::{CallSite, CodeItemKind};
use wiki_code::support::source_rank;

/// What a base a class head names is.
#[derive(Clone, Copy)]
enum Base<'a> {
    /// The project class of this path; whether the base's template
    /// arguments name a template parameter (a dependent base).
    Class(&'a str, bool),
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

/// The methods a lookup finds.
pub(super) struct Found {
    pub fns: BTreeSet<FnRef>,
    /// They may also be something outside: only pure virtual declarations
    /// were found, or one method of a class the caller's file does not
    /// settle.
    pub open: bool,
    /// Found only through dependent bases, which a bare call in the
    /// template does not search.
    pub dependent: bool,
}

/// The pages a caller's file sees: its own and its include closure.
#[derive(Clone, Copy)]
pub(super) struct Sight<'s> {
    pub page: usize,
    pub included: &'s BTreeSet<usize>,
}

impl Sight<'_> {
    fn sees(&self, page: usize) -> bool {
        page == self.page || self.included.contains(&page)
    }
}

#[derive(Default)]
pub(super) struct Classes<'a> {
    /// Per class whose head a file shows: per page holding a head, the
    /// bases that head names.
    heads: BTreeMap<&'a str, BTreeMap<usize, Vec<Base<'a>>>>,
    /// Per method of a class some head places: that class.
    placed: BTreeMap<FnRef, &'a str>,
    /// Per class: the project classes naming it as a base.
    derived: BTreeMap<&'a str, BTreeSet<&'a str>>,
    /// Per base name as written, by `owner_key`: the classes naming it.
    naming: BTreeMap<&'a str, BTreeSet<&'a str>>,
    /// Per class name (`owner_key`): the paths of the classes it names.
    by_key: BTreeMap<&'a str, BTreeSet<&'a str>>,
    /// Per alias name (`owner_key`): the page and path of each project
    /// alias of that name (`File::aliases`).
    aliases: BTreeMap<&'a str, Vec<(usize, &'a str)>>,
}

/// Where a written type name lands.
pub(super) enum Placed<'a> {
    /// This project class.
    Class(&'a str),
    /// One of these two: lookup among every class finds one, among those
    /// the file sees another.
    Either(&'a str, &'a str),
    /// Lookup passed something no head places (an alias, a class whose
    /// head no file shows, several candidates): any class, or something
    /// outside.
    Unplaced,
    /// No project class.
    Nothing,
}

/// One lookup's result, over one set of classes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step<'a> {
    Found(&'a str),
    Unplaced,
    Nothing,
}

/// `path` with its anonymous-namespace segments dropped: a file's own
/// anonymous namespaces are transparent to its code.
fn bare(path: &str) -> String {
    let segs = path.split("::").filter(|s| !s.starts_with("(anonymous@"));
    segs.collect::<Vec<_>>().join("::")
}

/// The include closure of a page that has none.
static NO_PAGES: BTreeSet<usize> = BTreeSet::new();

impl<'a> Cpp<'a> {
    /// Fills `classes` from every C/C++ file's heads, once the project's
    /// types and namespaces are known. Each head's bases are placed from
    /// what its own file sees, and so is each method's class.
    pub(super) fn read_classes(&mut self) {
        let pages = self.pages.iter().enumerate().map(|(p, f)| (p, f.1));
        let pages: Vec<_> = pages.filter(|f| f.1.family == Family::Cpp).collect();
        let heads = || {
            let all = pages
                .iter()
                .flat_map(|&(p, f)| f.bases.iter().map(move |h| (p, h)));
            all.map(|(p, (c, bases))| (p, c.as_str(), bases))
        };
        for (page, class, _) in heads() {
            self.types.insert(owner_key(class));
            let paths = self.classes.by_key.entry(owner_key(class)).or_default();
            paths.insert(class);
            let at = self.classes.heads.entry(class).or_default();
            at.insert(page, Vec::new());
        }
        for &(page, file) in &pages {
            for a in &file.aliases {
                let at = self.classes.aliases.entry(owner_key(a)).or_default();
                at.push((page, a.as_str()));
            }
        }
        let closures: BTreeMap<usize, BTreeSet<usize>> =
            pages.iter().map(|&(p, _)| (p, self.closure(p))).collect();
        let sight = |page: usize| Sight {
            page,
            included: closures.get(&page).unwrap_or(&NO_PAGES),
        };
        // A base is named from the scope around its class.
        let parent = |c: &'a str| c.rsplit_once("::").map_or("", |p| p.0);
        let read: Vec<(usize, &str, &str, Base)> = heads()
            .flat_map(|(page, c, bases)| bases.iter().map(move |b| (page, c, b)))
            .map(|(page, c, (b, dep))| {
                let base = match self.place_name(b, parent(c), sight(page)) {
                    Placed::Class(p) => Base::Class(p, *dep),
                    Placed::Either(..) | Placed::Unplaced => Base::Unseen,
                    Placed::Nothing => Base::Outside,
                };
                (page, c, b.as_str(), base)
            })
            .collect();
        for (page, class, written, base) in read {
            if let Base::Class(p, _) = base {
                self.classes.derived.entry(p).or_default().insert(class);
            }
            let naming = self.classes.naming.entry(owner_key(written));
            naming.or_default().insert(class);
            let at = self.classes.heads.entry(class).or_default();
            at.entry(page).or_default().push(base);
        }
        let methods = pages
            .iter()
            .flat_map(|&(p, f)| f.classes.keys().map(move |&i| (p, i)));
        let placed: Vec<(FnRef, &'a str)> = methods
            .filter_map(|(p, i)| Some(((p, i), self.place(p, i, sight(p))?)))
            .collect();
        self.classes.placed.extend(placed);
    }

    /// The class of the method item `i` of `page` (`File::classes`): the
    /// head of its path in the file's sight, else, unless the path names a
    /// namespace, the one class `place_name` gives.
    fn place(&self, page: usize, i: usize, sight: Sight) -> Option<&'a str> {
        let written = self.pages[page].1.classes.get(&i)?.as_str();
        let head = self.classes.heads.get_key_value(written);
        if let Some((&class, _)) = head.filter(|h| h.1.keys().any(|&p| sight.sees(p))) {
            return Some(class);
        }
        if self.namespaces.contains(owner_key(written)) {
            return None;
        }
        match self.place_name(written, "", sight) {
            Placed::Class(class) => Some(class),
            Placed::Either(..) | Placed::Unplaced | Placed::Nothing => None,
        }
    }

    /// Where the type written `t`, seen from the scope `from` (a path) in
    /// the file of `sight`, lands. The one entry for placing a written
    /// name: a base, a typed receiver, a qualifier, an out-of-line
    /// method's class.
    ///
    /// C++ lookup order: at each scope from `from` outward a project alias
    /// or class of that path (the file's `using` directives at the global
    /// scope), the class itself by its own name, then the aliases and
    /// classes nested in its nearest project bases (a base outside the
    /// project hides none; one no head places stops the lookup); last, the
    /// one class whose path ends in `t`. Run twice: over the classes an
    /// includer may provide too (every header's, outside its anonymous
    /// namespaces) besides those in files `sight` sees, and over those in
    /// files `sight` sees only; a class at one path counts once. Both find
    /// it, or only the first does: that class. They differ: either. An
    /// alias, a base no head places, or several classes met on the way:
    /// unplaced.
    pub(super) fn place_name(&self, t: &str, from: &str, sight: Sight) -> Placed<'a> {
        let t = untemplated(t);
        let t = t.trim_start_matches("::");
        if t.starts_with("std::") {
            return Placed::Nothing;
        }
        let seen = |page: usize, _: &str| sight.sees(page);
        // Another file's class or alias is reachable when an includer may
        // provide it: a header's, outside its anonymous namespaces.
        let reachable = |page: usize, path: &str| {
            let header = source_rank(self.pages[page].0) < 2;
            sight.sees(page) || (header && !path.contains("(anonymous@"))
        };
        let all = self.lookup_name(t, from, sight, &reachable);
        let mine = self.lookup_name(t, from, sight, &seen);
        match (all, mine) {
            (Step::Unplaced, _) | (_, Step::Unplaced) => Placed::Unplaced,
            (Step::Found(a), Step::Found(m)) if a != m => Placed::Either(a, m),
            (Step::Found(a), _) => Placed::Class(a),
            (Step::Nothing, Step::Found(m)) => Placed::Class(m),
            (Step::Nothing, Step::Nothing) => Placed::Nothing,
        }
    }

    /// `place_name`'s lookup over the classes and aliases `keep` lets in
    /// (by a page holding one and its path).
    fn lookup_name(
        &self,
        t: &str,
        from: &str,
        sight: Sight,
        keep: &dyn Fn(usize, &str) -> bool,
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
        let usings = &self.pages[sight.page].1.usings;
        let mut scope = from;
        loop {
            let mut wants = vec![match bare(scope).as_str() {
                "" => t.to_string(),
                s => format!("{s}::{t}"),
            }];
            if scope.is_empty() {
                // `using n::t;` names that one entity, before any other.
                let declared = usings.iter().filter(|u| u.rsplit("::").next() == Some(t));
                match at(&declared.cloned().collect::<Vec<_>>()) {
                    Step::Nothing => {}
                    found @ (Step::Found(_) | Step::Unplaced) => return found,
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
                    Base::Class(p, _) if seen.insert(p) => next.push(p),
                    // An outside base hides no project type name.
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

    /// The classes, but another file's anonymous-namespace ones, whose
    /// bare path is the type written `t` or ends in it: what an unplaced
    /// name may be.
    pub(super) fn namesakes(&self, t: &str, sight: Sight) -> Vec<&'a str> {
        let t = untemplated(t);
        let t = t.trim_start_matches("::");
        let ends = format!("::{t}");
        let heads = self.classes.by_key.get(owner_key(t)).into_iter().flatten();
        let named = heads.filter(|p| {
            let b = bare(p);
            let at = self
                .classes
                .heads
                .get(*p)
                .into_iter()
                .flat_map(|h| h.keys());
            let reachable = !p.contains("(anonymous@") || at.into_iter().any(|&pg| sight.sees(pg));
            (b == t || b.ends_with(&ends)) && reachable
        });
        named.copied().collect()
    }

    /// The class of the method item `i` of `page`: the one a head places;
    /// none for a namespace's function; else the path as written, which no
    /// head and no placed method has: a class lookup reads as unseen.
    pub fn class_of(&self, page: usize, i: usize) -> Option<&'a str> {
        if let Some(&class) = self.classes.placed.get(&(page, i)) {
            return Some(class);
        }
        let written = self.pages[page].1.classes.get(&i)?.as_str();
        (!self.namespaces.contains(owner_key(written))).then_some(written)
    }

    /// The one page holding a head of `class` that `sight` sees, when its
    /// heads are in several files.
    fn seen_head(&self, class: &str, sight: Option<Sight>) -> Option<usize> {
        let heads = self.classes.heads.get(class)?;
        let mut seen = heads.keys().filter(|&&p| sight.is_some_and(|s| s.sees(p)));
        match (heads.len(), seen.next(), seen.next()) {
            (2.., Some(&p), None) => Some(p),
            _ => None,
        }
    }

    /// Are the heads of `class` in one file, or does `sight` see one?
    pub(super) fn settled(&self, class: &str, sight: Option<Sight>) -> bool {
        let heads = self.classes.heads.get(class).map_or(0, BTreeMap::len);
        heads < 2 || self.seen_head(class, sight).is_some()
    }

    /// The methods `site` names that `class` defines, by argument count,
    /// folded: with heads in several files and one `sight` sees, those of
    /// that head (in its file or in one including it).
    pub(super) fn members(
        &self,
        class: &str,
        site: &CallSite,
        sight: Option<Sight>,
    ) -> BTreeSet<FnRef> {
        let only = self.seen_head(class, sight);
        let of = |c: FnRef| only.is_none_or(|h| c.0 == h || self.closure(c.0).contains(&h));
        let named = self.by_name.get(site.name.as_str()).into_iter().flatten();
        let own = named.copied().filter(|&c| {
            let it = self.item(c);
            let placed = self.classes.placed.get(&c);
            it.kind == CodeItemKind::Method && placed == Some(&class) && fits(it, site) && of(c)
        });
        // Declarations fold by owner name: drop another class's definitions.
        let folded = self.folded(own.collect()).into_iter();
        let mine = |c: &FnRef| self.classes.placed.get(c).is_none_or(|p| *p == class);
        folded.filter(|c| mine(c) && of(*c)).collect()
    }

    /// The bases of `class`: with heads in several files and one `sight`
    /// sees, that head's; else all.
    fn bases_of(&self, class: &str, sight: Sight) -> Option<Vec<Base<'a>>> {
        let heads = self.classes.heads.get(class)?;
        let all = match self.seen_head(class, Some(sight)) {
            Some(p) => heads.get(&p).into_iter().flatten().copied().collect(),
            None => heads.values().flatten().copied().collect(),
        };
        Some(all)
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
        all.flat_map(|c| self.members(c, site, None)).collect()
    }

    /// What a call of `site`'s name on an object of `class`, in the file
    /// `sight` sees from, may reach: the methods name lookup finds in the
    /// class or its nearest project bases, with each project subclass's
    /// own when `dispatch`; a pure virtual declaration only while no
    /// subclass defines the name.
    pub(super) fn lookup(
        &self,
        class: &'a str,
        site: &CallSite,
        sight: Sight,
        dispatch: bool,
    ) -> Result<Found, Missing> {
        let mut seen = BTreeSet::from([class]);
        let mut level = vec![(class, false)];
        let (mut missing, mut settled) = (Missing::Nowhere, true);
        while !level.is_empty() {
            let (mut found, mut dependent) = (BTreeSet::new(), true);
            for &(c, dep) in &level {
                settled &= self.settled(c, Some(sight));
                let fns = self.members(c, site, Some(sight));
                dependent &= dep || fns.is_empty();
                found.extend(fns);
            }
            if !found.is_empty() {
                let subs = self.descendants(class).into_iter().filter(|_| dispatch);
                let overrides: BTreeSet<FnRef> =
                    subs.flat_map(|c| self.members(c, site, None)).collect();
                let pure = overrides.is_empty() && found.iter().all(|&c| self.pure(c));
                if !overrides.is_empty() {
                    found.retain(|&c| !self.pure(c));
                }
                found.extend(overrides);
                let open = pure || (!settled && found.len() == 1);
                return Ok(Found {
                    fns: found,
                    open,
                    dependent,
                });
            }
            let mut next = Vec::new();
            for (c, dep) in level {
                let Some(bases) = self.bases_of(c, sight) else {
                    missing = Missing::Unseen;
                    continue;
                };
                for b in bases {
                    match b {
                        Base::Class(p, d) if seen.insert(p) => next.push((p, dep || d)),
                        Base::Class(..) => {}
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
