//! Calls on an object or a class: a data member's, a typed value's, a
//! qualifier's.

use super::classes::{Missing, Placed, Sight};
use super::{fits, Caller, Evidence};
use crate::flow::FlowScope;
use crate::model::MapCall;
use crate::source::{pointee, untemplated};
use std::collections::BTreeSet;
use wiki_code::index::owner_key;
use wiki_code::model::{CallSite, CodeItemKind};

/// The candidates of either of two classes a name may be, as found on
/// each: never certain.
pub(super) fn either(one: Evidence, other: Evidence) -> Evidence {
    let (mut fns, mut open) = (BTreeSet::new(), false);
    for e in [one, other] {
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
    open |= fns.len() < 2;
    Evidence::Found { fns, open }
}

/// `x` and the text after it, of a call written `this->x.f()` or
/// `this->x->f()` of the name `f`.
pub(super) fn this_field<'t>(text: &'t str, f: &str) -> Option<(&'t str, &'t str)> {
    let rest = text.strip_prefix("this")?.trim_start().strip_prefix("->")?;
    let rest = rest.trim_start();
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    let (x, after) = rest.split_at(end);
    let op = after.trim_start();
    let called = op.strip_prefix("->").or_else(|| op.strip_prefix('.'))?;
    let called = called.trim_start().strip_prefix(f)?.trim_start();
    (!x.is_empty() && called.starts_with(['(', '<'])).then_some((x, after))
}

impl Caller<'_, '_> {
    /// `x.f()` or `x->f()` on the data member `x` (`after` the text after
    /// it) of the caller's class: as on a local of the member's type, the
    /// type placed from the class declaring it, in the file of its head.
    pub(super) fn on_field(
        &self,
        chain: &[FlowScope],
        call: &MapCall,
        x: &str,
        after: &str,
    ) -> Evidence {
        let field = self
            .member_of(chain)
            .and_then(|c| self.cpp.field(c, x, self.sight()));
        match field.and_then(|f| Some((f.ty?, f))) {
            Some((t, f)) => {
                let included = self.cpp.closure(f.page);
                let head = Sight {
                    page: f.page,
                    included: &included,
                };
                // A nested class the head only declares (`struct Impl;`) is
                // defined where its members are called: in the caller's sight.
                let place = |sight| self.cpp.place_name(pointee(t).1, f.class, sight);
                let nested =
                    |p| matches!(p, Placed::Class(c) if c.starts_with(&format!("{}::", f.class)));
                let sight = if place(head) == Placed::Nothing && nested(place(self.sight())) {
                    self.sight()
                } else {
                    head
                };
                self.on_typed(call, after, t, f.class, sight)
            }
            None => self.any_method(call),
        }
    }

    /// A call of `f` on an object of the project class `class`: what
    /// lookup finds, else outside under a base from outside the project,
    /// else any project method `f`.
    pub(super) fn on_class(&self, call: &MapCall, class: &str) -> Evidence {
        match self.cpp.lookup(class, &call.site, self.sight(), true) {
            Ok(found) => Evidence::Found {
                fns: self.ids(found.fns),
                open: found.open,
            },
            Err(Missing::Outside) => Evidence::Outside,
            Err(Missing::Nowhere | Missing::Unseen) => self.any_method(call),
        }
    }

    /// `Q::f()` on the class `Q`: what lookup finds in it or its bases,
    /// with no dispatch; outside when an outside base may give it.
    pub(super) fn qualified(&self, call: &MapCall, class: &str) -> Evidence {
        match self.cpp.lookup(class, &call.site, self.sight(), false) {
            Ok(found) => Evidence::Found {
                fns: self.ids(found.fns),
                open: found.open,
            },
            Err(Missing::Outside) => Evidence::Outside,
            Err(Missing::Unseen) => self.any_method(call),
            Err(Missing::Nowhere) => Evidence::Hidden,
        }
    }

    /// `x.f()` or `x->f()` on `x` of the written type `t`, `after` the text
    /// after `x`. A smart pointer is itself with `.`, its pointee with
    /// `->`. A project class, found from the scope `from` in the file of
    /// `sight`: as `on_class`. A project type no head places: any project
    /// method `f`. Any other type: the project classes deriving from it
    /// that define `f`, or something outside.
    pub(super) fn on_typed(
        &self,
        call: &MapCall,
        after: &str,
        t: &str,
        from: &str,
        sight: Sight,
    ) -> Evidence {
        let (cpp, site) = (self.cpp, &call.site);
        let arrow = after.trim_start().starts_with("->");
        let (smart, t) = pointee(t);
        if smart && !arrow {
            return Evidence::Outside;
        }
        // `->` on a class with `operator->` (its own or a project base's)
        // reaches whatever that returns.
        let op = CallSite {
            name: "operator->".to_string(),
            args: None,
            ..site.clone()
        };
        let on = |class| match cpp.lookup(class, &op, self.sight(), false) {
            Ok(_) if arrow && !smart => self.any_method(call),
            Ok(_) | Err(_) => self.on_class(call, class),
        };
        match cpp.place_name(t, from, sight) {
            Placed::Class(class) => on(class),
            Placed::Either(one, other) => either(on(one), on(other)),
            Placed::Unplaced => self.any_method(call),
            Placed::Nothing => Evidence::Found {
                fns: self.ids(cpp.deriving(t, site)),
                open: true,
            },
        }
    }

    /// `Q::f()` with `Q` a project type names that one function, no
    /// dispatch: the member of the class `Q` lands on (`place_name`); of
    /// either class, never certain; unplaced, of the classes it may be, or
    /// something outside; with no class, the functions of a namespace `Q`,
    /// else the methods of an owner `Q`, or with none any project method
    /// `f`, or something outside.
    pub(super) fn on_qualifier(&self, call: &MapCall, q: &str) -> Evidence {
        let (cpp, site, sight) = (self.cpp, &call.site, self.sight());
        let callee = call.text.split('(').next().unwrap_or("");
        let callee = untemplated(callee);
        let written = callee.rsplit_once("::").map_or(q, |w| w.0);
        match cpp.place_name(written, self.class.unwrap_or(self.ns), sight) {
            Placed::Class(class) => return self.qualified(call, class),
            Placed::Either(one, other) => {
                return either(self.qualified(call, one), self.qualified(call, other));
            }
            Placed::Unplaced => {
                let named = cpp.namesakes(written, sight).into_iter();
                let fns = self.ids(named.flat_map(|c| cpp.members(c, site, None)));
                if !fns.is_empty() {
                    return Evidence::Found { fns, open: true };
                }
            }
            Placed::Nothing if cpp.namespaces.contains(q) => {
                return self.free(site, Some(q)).unwrap_or(Evidence::Hidden);
            }
            Placed::Nothing => {}
        }
        let named = cpp.by_name.get(site.name.as_str()).into_iter().flatten();
        let members = named.copied().filter(|&c| {
            let it = cpp.item(c);
            it.kind == CodeItemKind::Method
                && it.owner.as_deref().map(owner_key) == Some(q)
                && fits(it, site)
        });
        // An alias may name any class.
        let fns = self.ids(cpp.folded(members.collect()));
        if fns.is_empty() {
            return self.any_method(call);
        }
        Evidence::Found { fns, open: true }
    }
}
