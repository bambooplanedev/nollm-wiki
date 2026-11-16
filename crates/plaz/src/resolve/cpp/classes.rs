//! The classes of the project's C/C++ files, each by its path
//! (`File::classes`): what its bases are, which classes derive from it,
//! and what name lookup finds from it.

use super::{fits, Cpp};
use crate::source::untemplated;
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::{owner_key, Family, FnRef};
use wiki_code::model::{CallSite, CodeItemKind};

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
    /// Per type name: the pages declaring a type of that name.
    typed: BTreeMap<&'a str, BTreeSet<usize>>,
    /// Per type name: the pages declaring an alias of that name.
    aliases: BTreeMap<&'a str, BTreeSet<usize>>,
    /// The pages and names of classes in a page's anonymous namespace.
    anonymous: BTreeSet<(usize, &'a str)>,
}

/// `path` with its anonymous-namespace segments dropped: a file's own
/// anonymous namespaces are transparent to its code.
fn bare(path: &str) -> String {
    let segs = path.split("::").filter(|s| !s.starts_with("(anonymous@"));
    segs.collect::<Vec<_>>().join("::")
}

/// The one head among `cands` whose bare path is `want`; `Err` when
/// several are.
fn pick<'a>(cands: &[(&'a str, String)], want: &str) -> Result<Option<&'a str>, ()> {
    let mut hit = cands.iter().filter(|c| c.1 == want).map(|c| c.0);
    match (hit.next(), hit.next()) {
        (None, _) => Ok(None),
        (Some(p), None) => Ok(Some(p)),
        (Some(_), Some(_)) => Err(()),
    }
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
            if class.contains("(anonymous@") {
                self.classes.anonymous.insert((page, owner_key(class)));
            }
        }
        for &(page, file) in &pages {
            let types = file
                .code
                .items
                .iter()
                .filter(|it| it.kind == CodeItemKind::Type);
            for it in types {
                let pages = self.classes.typed.entry(&it.name).or_default();
                pages.insert(page);
                if it.signature.starts_with("using") || it.signature.starts_with("typedef") {
                    self.classes
                        .aliases
                        .entry(&it.name)
                        .or_default()
                        .insert(page);
                }
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
                (
                    page,
                    c,
                    b.as_str(),
                    self.base(b, parent(c), *dep, sight(page)),
                )
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

    /// What the base written `written` of a class in the scope `from` is,
    /// from what the class's file sees: a project type no head places when
    /// the project declares an alias of that name in view, when several
    /// classes in view may be it, or when a class out of view may be it;
    /// else outside.
    fn base(&self, written: &str, from: &str, dependent: bool, sight: Sight) -> Base<'a> {
        let key = owner_key(&untemplated(written)).to_string();
        let alias = self.classes.aliases.get(key.as_str());
        let alias = alias.is_some_and(|ps| ps.iter().any(|&p| sight.sees(p)));
        match self.resolve(written, from, sight) {
            Some(p) => Base::Class(p, dependent),
            None if alias || !self.namesakes(written, sight, true).is_empty() => Base::Unseen,
            None => Base::Outside,
        }
    }

    /// The class a head `sight` sees places for the method item `i` of
    /// `page`: the one of its path; else the one whose bare
    /// path is the written one or a `using` of the file completes it to;
    /// with none, unless the path names a namespace, as `resolve` places
    /// it.
    fn place(&self, page: usize, i: usize, sight: Sight) -> Option<&'a str> {
        let file = self.pages[page].1;
        let written = file.classes.get(&i)?.as_str();
        let head = self.classes.heads.get_key_value(written);
        if let Some((&class, _)) = head.filter(|h| h.1.keys().any(|&p| sight.sees(p))) {
            return Some(class);
        }
        let want = bare(written);
        let cands = self.candidates(owner_key(written), sight);
        let using = file.usings.iter().map(|u| format!("{u}::{want}"));
        let wants: BTreeSet<String> = using.chain([want.clone()]).collect();
        let hits: BTreeSet<&str> = (cands.iter())
            .filter(|c| wants.contains(&c.1))
            .map(|c| c.0)
            .collect();
        match hits.into_iter().collect::<Vec<_>>()[..] {
            [class] => Some(class),
            [] if !self.namespaces.contains(owner_key(written)) => self.resolve(written, "", sight),
            _ => None,
        }
    }

    /// The heads named `key` a name may be placed on from `sight`: those in
    /// the files it sees, each with its bare path.
    fn candidates(&self, key: &str, sight: Sight) -> Vec<(&'a str, String)> {
        let heads = self.classes.by_key.get(key).into_iter().flatten();
        let seen = heads.filter(|p| {
            let at = self
                .classes
                .heads
                .get(*p)
                .into_iter()
                .flat_map(|h| h.keys());
            at.into_iter().any(|&pg| sight.sees(pg))
        });
        seen.map(|&p| (p, bare(p))).collect()
    }

    /// The project class the type written `t` names from the scope `from`
    /// (a path), among the heads `sight` sees: at each scope from `from`
    /// outward, a class nested in it, the class itself by its own name,
    /// else a class nested in its nearest project bases (none past a base
    /// no head places); else the one class whose path ends in `t`.
    pub(super) fn resolve(&self, t: &str, from: &str, sight: Sight) -> Option<&'a str> {
        let t = untemplated(t);
        let t = t.trim_start_matches("::");
        if t.starts_with("std::") {
            return None;
        }
        let cands = self.candidates(owner_key(t), sight);
        let want = |s: &str| match s {
            "" => t.to_string(),
            s => format!("{s}::{t}"),
        };
        let mut scope = from;
        loop {
            match pick(&cands, &want(&bare(scope))) {
                Ok(None) => {}
                Ok(Some(p)) => return Some(p),
                Err(()) => return None,
            }
            // A class's own name in its body is the class.
            let own = self.classes.heads.get_key_value(scope).map(|h| *h.0);
            if let Some(p) = own.filter(|_| owner_key(scope) == t) {
                return Some(p);
            }
            match self.in_bases(scope, &|b| pick(&cands, &want(&bare(b))), sight) {
                Ok(None) => {}
                Ok(Some(p)) => return Some(p),
                Err(()) => return None,
            }
            if scope.is_empty() {
                break;
            }
            scope = scope.rsplit_once("::").map_or("", |s| s.0);
        }
        let ends = format!("::{t}");
        let mut named = cands.iter().filter(|c| c.1 == t || c.1.ends_with(&ends));
        match (named.next(), named.next()) {
            (Some(c), None) => Some(c.0),
            _ => None,
        }
    }

    /// What `nested(base)` places for the nearest project bases of the
    /// class `scope` (the bases of the head `sight` sees) that place one,
    /// `Ok(None)` when none does; `Err` when a base on the way is a
    /// project type no head places or one level has several.
    fn in_bases(
        &self,
        scope: &str,
        nested: &dyn Fn(&str) -> Result<Option<&'a str>, ()>,
        sight: Sight,
    ) -> Result<Option<&'a str>, ()> {
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
                    Base::Unseen => return Err(()),
                }
            }
            let mut found = BTreeSet::new();
            for b in &next {
                found.extend(nested(b)?);
            }
            match found.len() {
                0 => level = next,
                1 => return Ok(found.into_iter().next()),
                _ => return Err(()),
            }
        }
        Ok(None)
    }

    /// The classes whose bare path is the type written `t` or ends in it:
    /// those `sight` sees, and with `anywhere` those of every file but
    /// another file's anonymous namespace.
    pub(super) fn namesakes(&self, t: &str, sight: Sight, anywhere: bool) -> Vec<&'a str> {
        let t = untemplated(t);
        let t = t.trim_start_matches("::");
        let ends = format!("::{t}");
        let named = |p: &str| {
            let b = bare(p);
            b == t || b.ends_with(&ends)
        };
        let heads = self.classes.by_key.get(owner_key(t)).into_iter().flatten();
        let visible = |p: &str| {
            let at = self.classes.heads.get(p).into_iter().flat_map(|h| h.keys());
            at.into_iter()
                .any(|&pg| sight.sees(pg) || (anywhere && !p.contains("(anonymous@")))
        };
        heads.filter(|p| named(p) && visible(p)).copied().collect()
    }

    /// Is the type written `t` a project type's name, under no qualifier
    /// but a project namespace or type, declared in a file `sight` sees or
    /// outside any file's anonymous namespace?
    pub(super) fn is_type(&self, t: &str, sight: Sight) -> bool {
        let t = untemplated(t);
        let t = t.trim_start_matches("::");
        let root = t.split("::").next().unwrap_or("");
        let rooted =
            !t.contains("::") || self.namespaces.contains(root) || self.types.contains(root);
        let key = owner_key(t);
        let pages = self.classes.typed.get(key).into_iter().flatten();
        let declared = |&&p: &&usize| sight.sees(p) || !self.classes.anonymous.contains(&(p, key));
        root != "std" && rooted && pages.into_iter().any(|p| declared(&p))
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
    /// own; a pure virtual declaration only while no subclass defines the
    /// name.
    pub(super) fn lookup(
        &self,
        class: &'a str,
        site: &CallSite,
        sight: Sight,
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
                let subs = self.descendants(class).into_iter();
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
