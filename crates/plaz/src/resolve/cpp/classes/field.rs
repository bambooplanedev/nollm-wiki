//! The data members name lookup finds.

use super::{Base, Sight};
use crate::resolve::cpp::Cpp;
use std::collections::BTreeSet;

/// A data member name lookup finds.
pub(in crate::resolve::cpp) struct Field<'a> {
    /// Its written type (`CppFacts::fields`).
    pub ty: Option<&'a str>,
    /// The class declaring it, and the page holding that class's head.
    pub class: &'a str,
    pub page: usize,
}

impl<'a> Cpp<'a> {
    /// The data member `name` that lookup from `class` finds in it or its
    /// nearest project bases: none when no class on the way declares it,
    /// or when a class no head settles, a base no head places, a dependent
    /// base or two classes of one level may hold it instead.
    pub(in crate::resolve::cpp) fn field(
        &self,
        class: &'a str,
        name: &str,
        sight: Sight,
    ) -> Option<Field<'a>> {
        let mut seen = BTreeSet::from([class]);
        let mut level = vec![class];
        while !level.is_empty() {
            let mut found = Vec::new();
            for &c in &level {
                let heads = self.classes.heads.get(c)?;
                let page = match self.seen_head(c, Some(sight)) {
                    Some(p) => p,
                    None if heads.len() == 1 => *heads.keys().next()?,
                    None => return None,
                };
                let ty = self.pages[page]
                    .1
                    .cpp
                    .fields
                    .get(c)
                    .and_then(|f| f.get(name));
                found.extend(ty.map(|ty| Field {
                    ty: ty.as_deref(),
                    class: c,
                    page,
                }));
            }
            if !found.is_empty() {
                return found.pop().filter(|_| found.is_empty());
            }
            let mut next = Vec::new();
            let bases = level.iter().filter_map(|c| self.bases_of(c, sight));
            for b in bases.flatten() {
                match b {
                    Base::Class(p, false) if seen.insert(p) => next.push(p),
                    Base::Class(_, false) | Base::Outside => {}
                    Base::Class(_, true) | Base::Unseen => return None,
                }
            }
            level = next;
        }
        None
    }
}
