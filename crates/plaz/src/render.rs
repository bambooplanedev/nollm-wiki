//! The render stage: one Markdown page per code file. Source text reaches a
//! page only through the escaping function of its sink.

use crate::chart::{chart, coverage, relative};
use crate::flow::hooks;
use crate::model::{Bodiless, File, Flow, FnId, Project, Target};
use crate::names::Names;
use crate::resolve::Resolved;
use crate::write::Links;
use std::collections::BTreeMap;
use wiki_code::index::{owner_key, Family};

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

/// The pages of `r`, each function section with its level in the index
/// when `levels` has one.
pub fn render(
    r: &Resolved,
    names: &Names,
    links: &Links,
    levels: &BTreeMap<FnId, (usize, String)>,
) -> Vec<Page> {
    let p = r.project();
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
                    "\n## {}\n{} · {}\n",
                    names.anchor(&me),
                    inline_code(&it.signature),
                    links.code(path, it.start_line, it.end_line)
                ));
                if let Some((n, index)) = levels.get(&me) {
                    out.push_str(&format!("Level: [[{index}|{n}]]\n"));
                }
                out.push('\n');
                let Some(flow) = file.flows.get(&item) else {
                    out.push_str(&match file.cpp.declarations.get(&item) {
                        Some(Bodiless::Defaulted) => "Defaulted.\n".to_string(),
                        Some(Bodiless::Deleted) => "Deleted.\n".to_string(),
                        Some(Bodiless::Declared) | None => defined_at(r, names, &me),
                    });
                    continue;
                };
                out.push_str(&locals(flow));
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

/// The names the body binds, sorted by name, typed where the type is
/// known; parameters (in the signature) and `global`/`nonlocal` names (on
/// a line of their own) left out. Empty when there are none.
fn locals(flow: &Flow) -> String {
    let list = |names: Vec<String>, label: &str| {
        (!names.is_empty()).then(|| format!("{label}: {}\n", names.join(", ")))
    };
    let own = (flow.locals.iter())
        .filter(|n| !flow.params.contains(*n) && !flow.outside.contains(*n))
        .map(|n| match flow.types.get(n) {
            Some(t) => inline_code(&format!("{n}: {t}")),
            None => inline_code(n),
        })
        .collect();
    let outside = flow.outside.iter().map(|n| inline_code(n)).collect();
    let lines: String = [list(own, "Locals"), list(outside, "Outside names")]
        .into_iter()
        .flatten()
        .collect();
    if lines.is_empty() {
        lines
    } else {
        lines + "\n"
    }
}

/// `[[page#anchor|Qualified::name]]` of the function `f`.
pub(crate) fn link(p: &Project, names: &Names, f: &FnId) -> String {
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

/// The line of the declaration `me`'s section: its definitions
/// (`Resolved::definitions`).
fn defined_at(resolved: &Resolved, names: &Names, me: &FnId) -> String {
    let link = |d: &FnId| link(resolved.project(), names, d);
    let all: Vec<String> = resolved.definitions(me).iter().map(link).collect();
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
