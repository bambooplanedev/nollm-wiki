//! The render stage: one Markdown page per code file. Source text reaches a
//! page only through the escaping function of its sink.

use crate::chart::{chart, coverage, relative};
use crate::flow::hooks;
use crate::model::{Bodiless, File, Flow, FnId, Project, Target};
use crate::names::Names;
use crate::resolve::{fold_scope, namespace_owners, outer_scope, Resolved};
use crate::write::Links;
use std::collections::BTreeSet;
use wiki_code::index::{owner_key, Family, Index};
use wiki_code::model::{CallSite, CallVia};

pub use crate::chart::mermaid_label;

/// A code span holding `s` exactly: fenced with one more backtick than the
/// longest run in `s`, padded when `s` starts or ends with one. A line break
/// becomes a space, as it would inside a code span anyway.
pub fn inline_code(s: &str) -> String {
    let s = s.replace(['\r', '\n'], " ");
    let longest = s.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{s}{pad}{fence}")
}

/// `s` as the text of a `[[target|text]]` link: `|`, `[`, `]` and `#` (which
/// Quartz's link pattern excludes from the text) become their fullwidth
/// forms, a line break a space.
pub fn link_text(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '|' => '｜',
            '[' => '［',
            ']' => '］',
            '#' => '＃',
            '\r' | '\n' => ' ',
            _ => c,
        })
        .collect()
}

/// `s` as a double-quoted YAML scalar.
pub fn yaml_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub struct Page {
    /// Relative to the vault: `map/src/a_rs.md`.
    pub rel_path: String,
    pub content: String,
}

pub fn render(r: &Resolved, names: &Names, links: &Links) -> Vec<Page> {
    let p = r.project();
    let index = Index::new(&p.files);
    let owners = namespace_owners(p.files.values());
    p.files
        .iter()
        .map(|(path, file)| {
            let mut out = head(path, file, links);
            let mut items: Vec<usize> = file
                .flows
                .keys()
                .chain(file.cpp.declarations.keys())
                .copied()
                .collect();
            items.sort_unstable();
            for item in items {
                let it = &file.code.items[item];
                let me = FnId {
                    path: path.clone(),
                    item,
                };
                out.push_str(&format!(
                    "\n## {}\n{} · {}\n\n",
                    names.anchor(&me),
                    inline_code(&it.signature),
                    links.code(path, it.start_line, it.end_line)
                ));
                let Some(flow) = file.flows.get(&item) else {
                    out.push_str(&match file.cpp.declarations.get(&item) {
                        Some(Bodiless::Defaulted) => "Defaulted.\n".to_string(),
                        Some(Bodiless::Deleted) => "Deleted.\n".to_string(),
                        Some(Bodiless::Declared) | None => {
                            defined_at(&index, r, names, &owners, &me)
                        }
                    });
                    continue;
                };
                out.push_str(&called_from(p, names, &me));
                let href = |t: &FnId| {
                    format!(
                        "{}#{}",
                        relative(names.page(path), names.page(&t.path)),
                        names.anchor(t)
                    )
                };
                out.push_str(&format!(
                    "\n{}\n{}\n",
                    chart(&it.qualified_name(), path, flow, &href),
                    coverage(flow)
                ));
                out.push_str(&calls(p, names, flow));
            }
            Page {
                rel_path: format!("{}.md", names.page(path)),
                content: out,
            }
        })
        .collect()
}

/// `[[page#anchor|Qualified::name]]` of the function `f`.
fn link(p: &Project, names: &Names, f: &FnId) -> String {
    let it = &p.files[&f.path].code.items[f.item];
    format!(
        "[[{}#{}|{}]]",
        names.page(&f.path),
        names.anchor(f),
        link_text(&it.qualified_name())
    )
}

/// The front matter, the code link and the page's notes.
fn head(path: &str, file: &File, links: &Links) -> String {
    let (dir, name) = path.rsplit_once('/').map_or(("", path), |(d, n)| (d, n));
    let title = if dir.is_empty() {
        name.to_string()
    } else {
        format!("{name} · {dir}")
    };
    let mut out = format!(
        "---\ntitle: {}\ntags: [\"plaz\"]\nkind: map\nsource: {}\n---\n",
        yaml_str(&title),
        yaml_str(path)
    );
    out.push_str(&format!("Code: {}\n", links.code(path, 1, file.lines)));
    if file.syntax_errors {
        out.push_str("\nSyntax errors in this file: some functions may be missing from the map.\n");
    }
    if hooks(file.family).is_none() {
        out.push_str(&format!(
            "\nNot covered yet: {} flowcharts.\n",
            file.family.as_str()
        ));
    }
    if file.module_code {
        out.push_str("\nModule-level code is not charted.\n");
    }
    if file.test_fns > 0 {
        let s = if file.test_fns == 1 { "" } else { "s" };
        out.push_str(&format!(
            "\nTest code is not charted: {} function{s}.\n",
            file.test_fns
        ));
    }
    out
}

/// The **Called from** list of the charted function `me`.
fn called_from(p: &Project, names: &Names, me: &FnId) -> String {
    let file = &p.files[&me.path];
    let it = &file.code.items[me.item];
    let mut out = String::from("**Called from**\n");
    let callers = names.callers(me);
    // A C++ constructor, destructor or operator is mostly called by code
    // no call site shows.
    let unseen = file.family == Family::Cpp
        && (it.name.starts_with('~')
            || it.owner.as_deref().map(owner_key) == Some(it.name.as_str())
            || it
                .name
                .strip_prefix("operator")
                .is_some_and(|r| !r.starts_with(|c: char| c.is_alphanumeric() || c == '_')));
    if callers.is_empty() && unseen {
        out.push_str("- constructions and operator uses are not traced\n");
    } else if callers.is_empty() {
        out.push_str("- none found\n");
    }
    for (caller, line, certain) in callers {
        let how = if *certain {
            "certainly".to_string()
        } else {
            format!("may call (if this {} is this one)", inline_code(&it.name))
        };
        out.push_str(&format!("- {how}: {} L{line}\n", link(p, names, caller)));
    }
    out
}

/// The **Calls** list of `flow`.
fn calls(p: &Project, names: &Names, flow: &Flow) -> String {
    let link = |f: &FnId| link(p, names, f);
    let mut calls = Vec::new();
    flow.each_call(&mut |_, c| {
        let to = match &c.target {
            Target::Resolved(t) => link(t),
            Target::OneOf {
                candidates,
                outside,
            } => {
                let all: Vec<String> = candidates.iter().map(link).collect();
                let or = if *outside {
                    ", or outside the project"
                } else {
                    ""
                };
                format!("one of: {}{or}", all.join(", "))
            }
            Target::Outside => "outside the project".to_string(),
            Target::Hidden => "target not visible".to_string(),
        };
        calls.push(format!(
            "- {} L{} → {to}\n",
            inline_code(&c.text),
            c.site.line
        ));
    });
    if calls.is_empty() {
        calls.push("- none found\n".to_string());
    }
    format!("\n**Calls**\n{}", calls.concat())
}

/// The line of the declaration `me`'s section: the definition the index
/// pairs it with; else the charted functions of the same language, scope
/// (`fold_scope` or `outer_scope`, `owners` as `namespace_owners` gives) and name, only
/// those of its own file when it has any, in source order; never one
/// another file keeps to itself, nor a method of another class when heads
/// place both (`Resolved::class`).
fn defined_at(
    index: &Index<File>,
    resolved: &Resolved,
    names: &Names,
    owners: &BTreeSet<&str>,
    me: &FnId,
) -> String {
    let p = resolved.project();
    let file = &p.files[&me.path];
    let it = &file.code.items[me.item];
    let site = CallSite {
        name: it.name.clone(),
        via: CallVia::Bare,
        args: None,
        line: it.start_line,
    };
    let key = |d: &FnId| {
        let f = &p.files[&d.path];
        (fold_scope(f, d.item, owners), &f.code.items[d.item].name)
    };
    // One declared in an inline namespace may be defined in its parent,
    // unless another declaration is that one's: one written there, or one
    // of another inline namespace.
    let rival = |outer: &str| {
        let declared = p.files.values().flat_map(|f| {
            let all = f.cpp.declarations.iter();
            all.filter(|d| *d.1 == Bodiless::Declared)
                .map(move |d| (f, *d.0))
        });
        declared
            .filter(|&(f, i)| f.code.items[i].name == it.name)
            .map(|(f, i)| (f, i, fold_scope(f, i, owners)))
            .any(|(f, i, at)| {
                at != key(me).0 && (at == outer || outer_scope(f, i, at) == Some(outer))
            })
    };
    let outer = outer_scope(file, me.item, key(me).0)
        .filter(|o| !rival(o))
        .map(|s| (s, &it.name));
    // A `static` or anonymous-namespace function of another file is not it.
    let charted = |d: &FnId| {
        let f = &p.files[&d.path];
        d != me
            && f.flows.contains_key(&d.item)
            && (d.path == me.path || !f.cpp.local.contains(&d.item))
            && (key(d) == key(me) || Some(key(d)) == outer)
            && match (resolved.class(d), resolved.class(me)) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    };
    let paired = (index.page_of(&me.path).into_iter())
        .flat_map(|page| index.pick(page, &site, |c| c == (page, me.item)))
        .map(|(pg, item)| FnId {
            path: index.key(pg).to_string(),
            item,
        })
        .find(charted);
    // The index pairs by name: it cannot tell the parent's definition from
    // one at the inline namespace's own path.
    let defs = if let Some(d) = paired.filter(|_| outer.is_none()) {
        vec![d]
    } else {
        let mut all = Vec::new();
        for (path, f) in p.files.iter().filter(|(_, f)| f.family == file.family) {
            for &item in f.flows.keys() {
                let id = FnId {
                    path: path.clone(),
                    item,
                };
                if charted(&id) {
                    all.push((path.as_str(), f.code.items[item].start_line, id));
                }
            }
        }
        all.sort();
        if all.iter().any(|d| d.0 == me.path) {
            all.retain(|d| d.0 == me.path);
        }
        all.into_iter().map(|d| d.2).collect()
    };
    let all: Vec<String> = defs.iter().map(|d| link(p, names, d)).collect();
    match all.as_slice() {
        [] => "No definition in the project.\n".to_string(),
        [one] => format!("Defined at {one}\n"),
        _ => format!("Defined at one of: {}\n", all.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_code_fences_past_the_longest_backtick_run() {
        assert_eq!(inline_code("a"), "`a`");
        assert_eq!(inline_code("m.get(\"a`b\")"), "``m.get(\"a`b\")``");
        assert_eq!(inline_code("x``y`"), "``` x``y` ```");
        assert_eq!(inline_code("`a"), "`` `a ``");
        assert_eq!(inline_code("a\nb"), "`a b`");
    }

    #[test]
    fn link_text_has_nothing_that_ends_or_splits_a_link() {
        assert_eq!(
            link_text("<Vec<[u8; 4]> as F|G>#x\ny"),
            "<Vec<［u8; 4］> as F｜G>＃x y"
        );
    }

    #[test]
    fn yaml_str_escapes_quotes_backslashes_and_controls() {
        assert_eq!(yaml_str("it's \"x\"\\.rs"), "\"it's \\\"x\\\"\\\\.rs\"");
        assert_eq!(yaml_str("a\nb"), "\"a\\u000ab\"");
    }
}
