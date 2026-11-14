//! Page ids, heading anchors and callers, computed once before rendering.

use crate::model::{FnId, Target};
use crate::resolve::Resolved;
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::model::CodeItemKind;

pub struct Names {
    pages: BTreeMap<String, String>,
    anchors: BTreeMap<FnId, String>,
    callers: BTreeMap<FnId, Vec<(FnId, usize, bool)>>,
}

impl Names {
    /// The page id of the file at `path`, like `map/src/a_rs`.
    pub fn page(&self, path: &str) -> &str {
        &self.pages[path]
    }

    /// The heading anchor of `f` on its page.
    pub fn anchor(&self, f: &FnId) -> &str {
        &self.anchors[f]
    }

    /// Who calls `f`: (caller, line, certain), the certain ones first, each
    /// group sorted by caller and line.
    pub fn callers(&self, f: &FnId) -> &[(FnId, usize, bool)] {
        self.callers.get(f).map_or(&[], Vec::as_slice)
    }
}

/// Lowercased, every run of chars outside `[a-z0-9]` one `_`, no `_` at
/// either end; `fn` when nothing is left.
fn slug(s: &str) -> String {
    let lower = s.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        "fn".to_string()
    } else {
        words.join("_")
    }
}

/// `base`, else `base_2`, `base_3`, … : the first not in `taken`, then taken.
fn unique(base: &str, taken: &mut BTreeSet<String>) -> String {
    let (mut id, mut n) = (base.to_string(), 1);
    while taken.contains(&id) {
        n += 1;
        id = format!("{base}_{n}");
    }
    taken.insert(id.clone());
    id
}

pub fn names(r: &Resolved) -> Names {
    let p = r.project();
    let mut taken = BTreeSet::new();
    let pages = p
        .files
        .keys()
        .map(|path| {
            let base: String = path
                .to_lowercase()
                .chars()
                .map(|c| match c {
                    'a'..='z' | '0'..='9' | '_' | '/' | '-' => c,
                    _ => '_',
                })
                .collect();
            (path.clone(), unique(&format!("map/{base}"), &mut taken))
        })
        .collect();
    let mut anchors = BTreeMap::new();
    let mut callers: BTreeMap<FnId, BTreeSet<(bool, FnId, usize)>> = BTreeMap::new();
    for (path, file) in &p.files {
        // Every function a call can resolve to, so each target has one.
        let mut taken = BTreeSet::new();
        for (item, it) in file.code.items.iter().enumerate() {
            if matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method) && !it.nested && !it.test
            {
                let id = FnId {
                    path: path.clone(),
                    item,
                };
                anchors.insert(id, unique(&slug(&it.qualified_name()), &mut taken));
            }
        }
        for (&item, flow) in &file.flows {
            let caller = FnId {
                path: path.clone(),
                item,
            };
            flow.each_call(&mut |_, c| {
                let mut add = |t: &FnId, certain: bool| {
                    // `!certain` so the certain ones sort first.
                    callers.entry(t.clone()).or_default().insert((
                        !certain,
                        caller.clone(),
                        c.site.line,
                    ));
                };
                match &c.target {
                    Target::Resolved(t) => add(t, true),
                    Target::OneOf { candidates, .. } => {
                        for t in candidates {
                            add(t, false);
                        }
                    }
                    Target::Outside | Target::Hidden => {}
                }
            });
        }
    }
    let callers = callers
        .into_iter()
        .map(|(f, set)| {
            let v = set.into_iter().map(|(may, c, l)| (c, l, !may)).collect();
            (f, v)
        })
        .collect();
    Names {
        pages,
        anchors,
        callers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_of(files: &[(&str, &str)]) -> Names {
        let p = crate::source::project(files.iter().map(|(p, t)| (p.to_string(), t.to_string())));
        names(&crate::resolve::resolve(p))
    }

    #[test]
    fn page_ids_that_collide_take_a_suffix_in_path_order() {
        let n = names_of(&[
            ("x/a.b.rs", "fn f() {}\n"),
            ("x/a_b.rs", "fn f() {}\n"),
            ("src/A.rs", "fn f() {}\n"),
            ("src/a.rs", "fn f() {}\n"),
        ]);
        assert_eq!(n.page("x/a.b.rs"), "map/x/a_b_rs");
        assert_eq!(n.page("x/a_b.rs"), "map/x/a_b_rs_2");
        assert_eq!(n.page("src/A.rs"), "map/src/a_rs");
        assert_eq!(n.page("src/a.rs"), "map/src/a_rs_2");
    }

    #[test]
    fn slugs_drop_every_run_of_other_chars() {
        assert_eq!(slug("A.__init__"), "a_init");
        assert_eq!(slug("<Vec<[u8; 4]> as Foo>::m"), "vec_u8_4_as_foo_m");
        assert_eq!(slug("Störe::get"), "st_re_get");
        assert_eq!(slug("<>"), "fn");
    }
}
