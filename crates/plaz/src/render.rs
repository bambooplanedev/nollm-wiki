//! The render stage: one Markdown page per code file. Source text reaches a
//! page only through the escaping function of its sink.

use crate::chart::{chart, coverage, relative};
use crate::flow::hooks;
use crate::model::{FnId, Target};
use crate::names::Names;
use crate::resolve::Resolved;
use crate::write::Links;

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
    let link = |f: &FnId| {
        let it = &p.files[&f.path].code.items[f.item];
        format!(
            "[[{}#{}|{}]]",
            names.page(&f.path),
            names.anchor(f),
            link_text(&it.qualified_name())
        )
    };
    p.files
        .iter()
        .map(|(path, file)| {
            let (dir, name) = path
                .rsplit_once('/')
                .map_or(("", path.as_str()), |(d, n)| (d, n));
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
                out.push_str(
                    "\nSyntax errors in this file: some functions may be missing from the map.\n",
                );
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
            for (&item, flow) in &file.flows {
                let it = &file.code.items[item];
                let me = FnId {
                    path: path.clone(),
                    item,
                };
                out.push_str(&format!(
                    "\n## {}\n{} · {}\n\n**Called from**\n",
                    names.anchor(&me),
                    inline_code(&it.signature),
                    links.code(path, it.start_line, it.end_line)
                ));
                let callers = names.callers(&me);
                if callers.is_empty() {
                    out.push_str("- none found\n");
                }
                for (caller, line, certain) in callers {
                    let how = if *certain {
                        "certainly".to_string()
                    } else {
                        format!("may call (if this {} is this one)", inline_code(&it.name))
                    };
                    out.push_str(&format!("- {how}: {} L{line}\n", link(caller)));
                }
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
                out.push_str("\n**Calls**\n");
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
                out.push_str(&calls.concat());
            }
            Page {
                rel_path: format!("{}.md", names.page(path)),
                content: out,
            }
        })
        .collect()
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
