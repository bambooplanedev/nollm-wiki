//! The free functions a call may name: by tier, in view of the caller.

use super::{fits, fold_scope, outer_scope, Caller, Evidence};
use wiki_code::index::FnRef;
use wiki_code::model::{CallSite, CodeItem, CodeItemKind};
use wiki_code::support::source_rank;

impl Caller<'_, '_> {
    /// The free functions `site` names, of a namespace whose path ends in
    /// `ns` (the qualifier as written) when given, or is it after a leading
    /// `::`: in C those of the
    /// caller's file first; those of the caller's file and the files it
    /// includes; else any other not local to its file (in a header, called
    /// from a header, it may be), which may also be something outside. A
    /// bare call's set keeps the functions in view, or with none in view
    /// is open. None by argument count: the same over the variadic ones.
    pub(super) fn free(&self, site: &CallSite, ns: Option<&str>) -> Option<Evidence> {
        let cpp = self.cpp;
        // `::a` is `a` of the global namespace only; `::` that namespace.
        let exact = ns.is_some_and(|w| w.starts_with("::"));
        // `k::b` under `namespace k = a;` is `a::b`.
        let ns = ns.map(|w| {
            let w = w.trim_start_matches("::");
            let (root, rest) = w.split_once("::").map_or((w, None), |(r, t)| (r, Some(t)));
            match (self.aliased(root), rest) {
                (Some(a), Some(rest)) => format!("{a}::{rest}"),
                (Some(a), None) => a.to_string(),
                (None, _) => w.to_string(),
            }
        });
        let ns = ns.as_deref();
        let in_ns = |c: FnRef| match ns {
            None => cpp.item(c).kind == CodeItemKind::Fn,
            // `int a::core::run(int)` is `run` of the namespace `a::core`.
            // One in an inline namespace is its parent's by name too.
            Some(q) => {
                let file = cpp.pages[c.0].1;
                let at = fold_scope(file, c.1, &cpp.owners);
                let named = |at: &str| at == q || (!exact && at.ends_with(&format!("::{q}")));
                named(at) || outer_scope(file, c.1, at).is_some_and(named)
            }
        };
        let named = cpp
            .by_name
            .get(site.name.as_str())
            .map_or(&[][..], Vec::as_slice);
        let by_arity = |it: &CodeItem| fits(it, site);
        let variadic = |it: &CodeItem| it.signature.contains("...");
        for keep in [&by_arity as &dyn Fn(&CodeItem) -> bool, &variadic] {
            let all: Vec<FnRef> = (named.iter().copied())
                .filter(|&c| in_ns(c) && keep(cpp.item(c)))
                .collect();
            let tier = |pick: &dyn Fn(FnRef) -> bool| -> Vec<FnRef> {
                all.iter().copied().filter(|&c| pick(c)).collect()
            };
            let seen = |c: FnRef| self.included.contains(&c.0);
            // A header's file-local function may be in every unit that
            // includes the calling header too.
            let header = |page: usize| source_rank(cpp.pages[page].0) < 2;
            let reachable = |c: FnRef| !cpp.local(c) || (header(self.page) && header(c.0));
            let own = |c: FnRef| c.0 == self.page;
            // In C a name has one signature: the caller's own is the one.
            let c_file = cpp.is_c(self.page);
            let tiers = [
                (tier(&|c| c_file && own(c)), false),
                (tier(&|c| own(c) || seen(c)), false),
                (tier(&|c| !own(c) && !seen(c) && reachable(c)), true),
            ];
            for (fns, open) in tiers {
                // A bare call sees a namespace's function from in it or
                // through a `using`; argument-dependent lookup, any.
                let shown: Vec<FnRef> = (fns.iter().copied())
                    .filter(|&c| ns.is_some() || self.in_view(c, &site.name))
                    .collect();
                let (fns, open) = if shown.is_empty() {
                    (fns, true)
                } else {
                    (shown, open)
                };
                let (fns, loose) = cpp.folded(fns);
                let open = open || loose.open(cpp.defined_in(&fns, self.page));
                let fns = self.ids(fns);
                if !fns.is_empty() {
                    return Some(Evidence::Found { fns, open });
                }
            }
        }
        None
    }

    /// Is the function `c` in view of a bare call of `name` here: in no
    /// namespace, in the caller's or an enclosing one, or named by one of
    /// the file's `using`s: exactly by one at file scope only, else by
    /// the end of its path (an inner one may name a nested namespace)?
    pub(super) fn in_view(&self, c: FnRef, name: &str) -> bool {
        let facts = &self.cpp.pages[c.0].1.cpp;
        let at = facts.scopes.get(&c.1).map_or("", String::as_str);
        let encloses = self.ns == at || self.ns.starts_with(&format!("{at}::"));
        let file = self.file;
        // A `using` may name the inline namespace the function is in.
        let inline = facts.inline_scopes.get(&c.1).map(String::as_str);
        let names = |u: &str, exact: bool| {
            let named = |at: &str| at == u || (!exact && at.ends_with(&format!("::{u}")));
            named(at) || inline.is_some_and(named)
        };
        let using = |u: &String| {
            let exact = file.cpp.file_usings.contains(u) && !file.cpp.inner_usings.contains(u);
            let of = u.strip_suffix(name).and_then(|n| n.strip_suffix("::"));
            names(u, exact) || of.is_some_and(|n| names(n, exact))
        };
        at.is_empty() || encloses || file.cpp.usings.iter().any(using)
    }
}
