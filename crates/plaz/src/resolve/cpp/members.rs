//! Calls on an object or a class: a data member's, a typed value's, a
//! qualifier's.

use super::classes::{Field, Missing, Placed, Sight};
use super::{fits, Caller, Evidence};
use crate::flow::FlowScope;
use crate::model::MapCall;
use crate::source::pointee;
use wiki_code::index::owner_key;
use wiki_code::model::{CallSite, CodeItemKind};

/// The candidates of either of two classes a name may be, as found on
/// each: never certain.
pub(super) fn either(one: Evidence, other: Evidence) -> Evidence {
    let Evidence::Found { fns, open } = one.and(other, false) else {
        return Evidence::Hidden;
    };
    Evidence::Found {
        open: open || fns.len() < 2,
        fns,
    }
}

/// The data member `a` and the text after it, of the text after a
/// receiver that goes on past `a` (`.a.f()`, `->a->f()`).
fn member_step(after: &str) -> Option<(&str, &str)> {
    let op = after.trim_start();
    let rest = op.strip_prefix("->").or_else(|| op.strip_prefix('.'))?;
    let rest = rest.trim_start();
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    let (a, tail) = rest.split_at(end);
    let tail = tail.trim_start();
    let on = tail.starts_with('.') || tail.starts_with("->");
    (!a.is_empty() && on).then_some((a, tail))
}

/// The first name `x` and the text after it, of a callee that is a path
/// of names ending in `f` (`x.f`, `x->a.f`, `this->a.f<T>`).
fn member_path<'t>(callee: &'t str, f: &str) -> Option<(&'t str, &'t str)> {
    let end = callee.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    let (x, after) = callee.split_at(end);
    let mut rest = after;
    while let Some((_, tail)) = member_step(rest) {
        rest = tail;
    }
    let op = rest.trim_start();
    let name = op.strip_prefix("->").or_else(|| op.strip_prefix('.'))?;
    let past = name.trim_start().strip_prefix(f)?.trim_start();
    (!x.is_empty() && (past.is_empty() || past.starts_with('<'))).then_some((x, after))
}

impl Caller<'_, '_, '_> {
    /// `x.f()`, `x->a.f()` (`x` as the call site gives it, or none):
    /// the innermost flow binding `x` gives its type; with none, a data
    /// member of that name. `this->a.f()`: the caller's class.
    pub(super) fn cpp_member(
        &self,
        chain: &[FlowScope],
        call: &MapCall,
        x: Option<&str>,
    ) -> Evidence {
        let path = match x {
            Some(x) => Some((x, call.text.strip_prefix(x).unwrap_or(""))),
            None => member_path(call.callee.as_deref().unwrap_or(""), &call.site.name),
        };
        let Some((x, after)) = path else {
            return self.any_method(call);
        };
        match chain.iter().rev().find(|f| f.locals.contains(x)) {
            _ if x == "this" => match self.member_of(chain) {
                Some(class) => self.on_step(call, after, class),
                None => self.any_method(call),
            },
            Some(f) => match f.types.get(x) {
                Some(t) => {
                    let from = self.class.unwrap_or(self.ns);
                    let local = self.on_typed(call, after, t, from, self.sight());
                    self.or_field(chain, f, call, x, Some(after), local)
                }
                None => self.any_method(call),
            },
            None => self.on_field(chain, call, x, Some(after)),
        }
    }

    /// `x.f()` or `x->f()` on the data member `x` (`after` the text after
    /// it) of the caller's class, or `x(…)` with no `after`: as `on_member`.
    pub(super) fn on_field(
        &self,
        chain: &[FlowScope],
        call: &MapCall,
        x: &str,
        after: Option<&str>,
    ) -> Evidence {
        let field = self
            .member_of(chain)
            .and_then(|c| self.cpp.field(c, x, self.sight()));
        self.on_member(call, field, after)
    }

    /// `local`, a call on or of (no `after`) the local `x` the flow `f`
    /// binds: of either it or the data member `x` when `f` binds `x` only
    /// in an inner block, where alone it hides the member.
    pub(super) fn or_field(
        &self,
        chain: &[FlowScope],
        f: &FlowScope,
        call: &MapCall,
        x: &str,
        after: Option<&str>,
        local: Evidence,
    ) -> Evidence {
        let class = self.member_of(chain).filter(|_| f.inner.contains(x));
        match class.and_then(|c| self.cpp.field(c, x, self.sight())) {
            Some(field) => either(local, self.on_member(call, Some(field), after)),
            None => local,
        }
    }

    /// A call on the data member `field` (`after` the text after it), or
    /// of it with no `after`: as on a local of the member's type, the type
    /// placed from the class declaring it, in the file of its head. With
    /// no member or no type: called itself, not visible; called on, any
    /// project method of the call's name.
    fn on_member(&self, call: &MapCall, field: Option<Field>, after: Option<&str>) -> Evidence {
        match field.and_then(|f| Some((f.ty?, f))) {
            Some((t, f)) => {
                let head = Sight {
                    page: f.page,
                    included: self.cpp.closure(f.page),
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
                match after {
                    Some(after) => self.on_typed(call, after, t, f.class, sight),
                    None => self.called(call, t, f.class, sight),
                }
            }
            None if after.is_none() => Evidence::Hidden,
            None => self.any_method(call),
        }
    }

    /// `x(…)` on `x` of the written type `t`, placed from the scope `from`
    /// in the file of `sight`: the `operator()` of that project class (or
    /// either class), its subclasses' too. A smart pointer, a function
    /// pointer, `std::function`, an unplaced name, a class with no
    /// `operator()`: not visible.
    pub(super) fn called(&self, call: &MapCall, t: &str, from: &str, sight: Sight) -> Evidence {
        let op = MapCall {
            site: CallSite {
                name: "operator()".to_string(),
                ..call.site.clone()
            },
            ..call.clone()
        };
        let on = |class| match self.cpp.lookup(class, &op.site, self.sight(), true) {
            Err(Missing::Nowhere) => Evidence::Hidden,
            Ok(_) | Err(Missing::Outside | Missing::Unseen) => self.on_class(&op, class),
        };
        if pointee(t).0 {
            return Evidence::Hidden;
        }
        match self.cpp.place_name(t, from, sight) {
            Placed::Class(class) => on(class),
            Placed::Either(one, other) => either(on(one), on(other)),
            Placed::Unplaced | Placed::Nothing => Evidence::Hidden,
        }
    }

    /// A call of `f` on an object of the project class `class`: what
    /// lookup finds, else a data member `f` lookup finds (`on_member`),
    /// else outside under a base from outside the project, else any
    /// project method `f`.
    pub(super) fn on_class(&self, call: &MapCall, class: &str) -> Evidence {
        match self.cpp.lookup(class, &call.site, self.sight(), true) {
            Ok(found) => self.found(found),
            Err(missing) => self.on_data(call, class).unwrap_or_else(|| match missing {
                Missing::Outside => Evidence::Outside,
                Missing::Nowhere | Missing::Unseen => self.any_method(call),
            }),
        }
    }

    /// `Q::f()` on the class `Q`: what lookup finds in it or its bases,
    /// with no dispatch; else a data member `f` lookup finds; outside when
    /// an outside base may give it.
    pub(super) fn qualified(&self, call: &MapCall, class: &str) -> Evidence {
        match self.cpp.lookup(class, &call.site, self.sight(), false) {
            Ok(found) => self.found(found),
            Err(missing) => self.on_data(call, class).unwrap_or_else(|| match missing {
                Missing::Outside => Evidence::Outside,
                Missing::Unseen => self.any_method(call),
                Missing::Nowhere => Evidence::Hidden,
            }),
        }
    }

    /// A call of the data member of the call's name that lookup from
    /// `class` finds, if one: its own call operator.
    fn on_data(&self, call: &MapCall, class: &str) -> Option<Evidence> {
        let field = self.cpp.field(class, &call.site.name, self.sight())?;
        Some(self.on_member(call, Some(field), None))
    }

    /// `x.a.f()` on an object of `class`, `after` the text after `x`: with
    /// a data member `a` before the called name, as on the member (`a`
    /// untyped or no data member: any project method `f`); else as
    /// `on_class`.
    fn on_step(&self, call: &MapCall, after: &str, class: &str) -> Evidence {
        let Some((a, tail)) = member_step(after) else {
            return self.on_class(call, class);
        };
        let field = self.cpp.field(class, a, self.sight());
        match field.filter(|f| f.ty.is_some()) {
            Some(f) => self.on_member(call, Some(f), Some(tail)),
            None => self.any_method(call),
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
            Ok(_) | Err(_) => self.on_step(call, after, class),
        };
        match cpp.place_name(t, from, sight) {
            Placed::Class(class) => on(class),
            Placed::Either(one, other) => either(on(one), on(other)),
            Placed::Unplaced => self.any_method(call),
            // `x.a.f()`: `a` may be a project class's.
            Placed::Nothing if member_step(after).is_some() => self.any_method(call),
            Placed::Nothing => Evidence::Found {
                fns: self.ids(cpp.deriving(t, site)),
                open: true,
            },
        }
    }

    /// `Q::f()` (`written` its qualifier, `Q` its last segment) with `Q`
    /// a project type names that one function, no
    /// dispatch: the member of the class `Q` lands on (`place_name`); of
    /// either class, never certain; unplaced, of the classes it may be, or
    /// something outside; with no class, the functions of a namespace `Q`,
    /// else the methods of an owner `Q`, or with none any project method
    /// `f`, or something outside.
    pub(super) fn on_qualifier(&self, call: &MapCall, q: &str, written: &str) -> Evidence {
        let (cpp, site, sight) = (self.cpp, &call.site, self.sight());
        match cpp.place_name(written, self.class.unwrap_or(self.ns), sight) {
            Placed::Class(class) => return self.qualified(call, class),
            Placed::Either(one, other) => {
                return either(self.qualified(call, one), self.qualified(call, other));
            }
            Placed::Unplaced => {
                let named = cpp.namesakes(written, sight).into_iter();
                let fns = self.ids(named.flat_map(|c| cpp.members(c, site, None).0));
                if !fns.is_empty() {
                    return Evidence::Found { fns, open: true };
                }
            }
            Placed::Nothing if cpp.namespaces.contains(q) => {
                return self.free(site, Some(written)).unwrap_or(Evidence::Hidden);
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
        let fns = self.ids(cpp.folded(members.collect()).0);
        if fns.is_empty() {
            return self.any_method(call);
        }
        Evidence::Found { fns, open: true }
    }
}
