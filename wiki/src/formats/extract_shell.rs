//! Shell (`.sh`, `.bash`): functions only, each listed as `name()`. The doc of
//! a function is the `#` run directly above it; a file the grammar can't fully
//! parse gets its functions from a line regex (`missed_functions`).
use super::code::{LangSpec, Lead};
use crate::model::{CodeItem, CodeItemKind};
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Node;

/// Functions, plus the argument of `source` / `.` / `builtin source` as `@import`.
const QUERY: &str = r#"
(function_definition name: (word) @name) @def
((command name: (command_name) @cmd . argument: (_) @import)
  (#any-of? @cmd "source" "." "\\."))
((command name: (command_name) @cmd . argument: (word) @sub . argument: (_) @import)
  (#eq? @cmd "builtin") (#eq? @sub "source"))
"#;

pub(crate) fn shell_spec() -> LangSpec {
    LangSpec {
        item_lead: shell_lead,
        fixed_signature: |_, name| Some(format!("{name}()")),
        ..LangSpec::base("shell", tree_sitter_bash::LANGUAGE.into(), QUERY)
    }
}

/// A `#` comment line is a doc line; a shebang, a divider (no letters or
/// digits) and anything that is not a comment end the run. A `# shellcheck`
/// directive instructs a tool and stays out of the doc.
fn shell_lead(node: Node, text: &str) -> Lead {
    let body = match text
        .get(node.byte_range())
        .and_then(|t| t.strip_prefix('#'))
    {
        Some(b) if node.kind() == "comment" && !b.starts_with('!') => b.trim(),
        _ => return Lead::Stop,
    };
    if !body.chars().any(char::is_alphanumeric) {
        Lead::Stop
    } else if body.starts_with("shellcheck ") {
        Lead::Attr
    } else {
        Lead::Doc(body.to_string())
    }
}

/// `name() {`, `function name {`, `function name() (`: a function opening line.
static OPENING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(function\s+)?([A-Za-z_][\w:.-]*)\s*(\(\s*\))?\s*[{(]").unwrap()
});

/// For a file with parse errors: the functions the line regex finds that
/// `found` (the tree's items) lacks. A function spans from its line to the next
/// line starting with `}` at column 0, or to the end of the file; a one-liner
/// ends on its own line. These items carry no doc.
pub(crate) fn missed_functions(text: &str, found: &[CodeItem]) -> Vec<CodeItem> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<CodeItem> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(c) = OPENING.captures(line) else {
            continue;
        };
        // `if (` and `echo (` open no function: it takes `function` or `()`.
        if c.get(1).is_none() && c.get(3).is_none() {
            continue;
        }
        let name = &c[2];
        if found.iter().chain(&out).any(|it| it.name == name) {
            continue;
        }
        let end = if line.trim_end().ends_with('}') {
            i
        } else {
            lines[i + 1..]
                .iter()
                .position(|l| l.starts_with('}'))
                .map_or(lines.len() - 1, |p| i + 1 + p)
        };
        out.push(CodeItem {
            kind: CodeItemKind::Fn,
            name: name.to_string(),
            owner: None,
            signature: format!("{name}()"),
            start_line: i + 1,
            end_line: end + 1,
            doc: None,
            doc_text: None,
            public: true,
            twin: None,
            test: false,
            calls: Vec::new(),
            call_lines: Vec::new(),
            call_sites: Vec::new(),
            arity: None,
        });
    }
    out
}

/// A shell file's `<top level>` item, spanning lines `1..=last`; the caller
/// of every call outside a function.
pub(crate) fn top_level_item(last: usize) -> CodeItem {
    CodeItem {
        kind: CodeItemKind::TopLevel,
        name: "<top level>".to_string(),
        owner: None,
        signature: "<top level>".to_string(),
        start_line: 1,
        end_line: last,
        doc: None,
        doc_text: None,
        public: false,
        twin: None,
        test: false,
        calls: Vec::new(),
        call_lines: Vec::new(),
        call_sites: Vec::new(),
        arity: None,
    }
}
