//! Shell (`.sh`, `.bash`): functions only, each listed as `name()`. The doc of
//! a function is the `#` run directly above it; a file the grammar can't fully
//! parse is parsed again from a blanked copy (`reparse`), and gets the
//! functions still missing from a line regex (`missed_functions`).
use super::code::{LangSpec, Lead};
use crate::model::{CodeItem, CodeItemKind};
use regex::bytes::Regex as BytesRegex;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::{Node, Parser, Tree};

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

/// For a tree with ERROR nodes: the tree of `blanked(text)` when it has fewer
/// ERROR lines, else `tree`. Its nodes index `text` itself: the copy has the
/// same length and changes only ASCII bytes, never a newline.
pub(crate) fn reparse(parser: &mut Parser, text: &str, tree: Tree) -> Tree {
    if !tree.root_node().has_error() {
        return tree;
    }
    let copy = blanked(text);
    match parser.parse(&copy, None) {
        Some(t) if error_lines(t.root_node()) < error_lines(tree.root_node()) => t,
        _ => tree,
    }
}

/// Lines covered by the outermost ERROR nodes.
fn error_lines(n: Node) -> usize {
    if n.is_error() {
        return n.end_position().row - n.start_position().row + 1;
    }
    n.children(&mut n.walk()).map(error_lines).sum()
}

static CASE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?:^|[\s;(])case\s.*\sin(?:\s|$)").unwrap());
static ESAC: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?:^|[\s;])esac\b").unwrap());
static HEREDOC: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r#"(?:^|[^<])<<-?\s*["']?([A-Za-z_][A-Za-z0-9_]*)"#).unwrap());
/// A case item's pattern: the text up to the first `)` before a blank.
static ITEM: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"^\s*(?:\|\s*)?\(?\s*(.*?)\)(?:\s|$)").unwrap());
/// Words of plain, quoted or escaped text joined by `|`.
static PATTERN: LazyLock<BytesRegex> = LazyLock::new(|| {
    let w = r#"(?:"[^"]*"|'[^']*'|\\.|[^\s"'|()])+"#;
    BytesRegex::new(&format!(r"^(?:{w}\s*\|\s*)*{w}$")).unwrap()
});
static COMMAND: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(r"^\s*(?:if|while|until|for|case|echo|printf|local|export|return)\b").unwrap()
});
static EXPANSION: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(r"\$\{#?(?:[A-Za-z_][A-Za-z0-9_]*|[0-9@*#?$!-])(?:\[[^\]]*\])?").unwrap()
});

/// `text` with what tree-sitter-bash 0.25 rejects though bash takes it turned
/// to `_`: a case pattern holding a glob and a quote, `[ ] {` in a
/// `${v#pat}` / `${v%pat}` / `${v/pat}` pattern, `;` in a `${v:+word}` word.
/// Comments and here-document bodies stay as they are.
fn blanked(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let (mut depth, mut heredoc) = (0usize, None::<Vec<u8>>);
    for line in text.as_bytes().split_inclusive(|&c| c == b'\n') {
        let mut b = line.to_vec();
        if let Some(h) = &heredoc {
            if line.trim_ascii() == h.as_slice() {
                heredoc = None;
            }
        } else if !line.trim_ascii_start().starts_with(b"#") {
            if depth > 0 {
                blank_case_pattern(&mut b);
            }
            let opened = CASE.find(&b).map(|m| m.end());
            depth = (depth + CASE.find_iter(&b).count()).saturating_sub(ESAC.find_iter(&b).count());
            if let Some(e) = opened {
                blank_case_pattern(&mut b[e..]);
            }
            blank_expansions(&mut b);
            heredoc = HEREDOC.captures(&b).map(|c| c[1].to_vec());
        }
        out.extend(b);
    }
    debug_assert_eq!(out.len(), text.len());
    out
}

fn blank_case_pattern(b: &mut [u8]) {
    let Some(r) = ITEM.captures(b).and_then(|c| c.get(1)).map(|g| g.range()) else {
        return;
    };
    let pat = &b[r.clone()];
    let has = |set: &[u8]| pat.iter().any(|c| set.contains(c));
    if !has(b"\"'")
        || !has(b"*?[")
        || has(b"`")
        || pat.windows(2).any(|w| w == b"$(")
        || !PATTERN.is_match(pat)
        || COMMAND.is_match(pat)
    {
        return;
    }
    let mut quote = None;
    for c in &mut b[r] {
        match quote {
            Some(q) if *c == q => quote = None,
            None if *c == b'"' || *c == b'\'' => quote = Some(*c),
            None if b"| \t".contains(c) => continue,
            _ => {}
        }
        if c.is_ascii() {
            *c = b'_';
        }
    }
}

fn blank_expansions(b: &mut [u8]) {
    let starts: Vec<usize> = EXPANSION.find_iter(b).map(|m| m.end()).collect();
    for i in starts {
        let Some(e) = close(b, i) else { continue };
        match b[i] {
            b'#' | b'%' | b'/' => blank_in(b, i + 1, e, b"[]{"),
            b':' if i + 1 < e && b"-+=?".contains(&b[i + 1]) => blank_in(b, i + 2, e, b";"),
            _ => {}
        }
    }
}

/// Index of the `}` closing the `${` whose body starts at `i`.
fn close(b: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'$' if b.get(i + 1) == Some(&b'{') => (depth, i) = (depth + 1, i + 1),
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

/// `set` bytes in `b[i..j]` to `_`, nested `${…}` and `$(…)` left alone.
fn blank_in(b: &mut [u8], mut i: usize, j: usize, set: &[u8]) {
    while i < j {
        match (b[i], b.get(i + 1).copied()) {
            (b'\\', Some(n)) if i + 1 < j => {
                if set.contains(&n) {
                    b[i + 1] = b'_';
                }
                i += 1;
            }
            (b'$', Some(b'{')) if i + 1 < j => i = close(b, i + 2).unwrap_or(j),
            (b'$', Some(b'(')) if i + 1 < j => {
                let mut depth = 0;
                i += 1;
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 1,
                        b'(' => depth += 1,
                        b')' if depth == 1 => break,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            (c, _) if set.contains(&c) => b[i] = b'_',
            _ => {}
        }
        i += 1;
    }
}
