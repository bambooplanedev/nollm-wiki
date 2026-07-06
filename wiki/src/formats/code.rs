use crate::formats::{summarize, Extractor};
use crate::model::{slugify, Entity, SourceKind};
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

pub struct CodeExtractor;

struct LangSpec {
    lang_name: &'static str,
    language: Language,
    /// Query capturing:
    ///   @def    - the definition node whose text is shortened to a signature
    ///   @name   - the definition's name (used for post-hoc export/visibility filtering)
    ///   @import - a module path node
    query_src: &'static str,
    /// Keep a captured definition only if its @name text passes this filter.
    /// Used where the grammar can't express the gate structurally (Python's
    /// leading-underscore convention, Go's capitalized-identifier convention).
    name_filter: fn(&str) -> bool,
    /// Trailing characters to strip off a built signature (e.g. Rust's `;`
    /// on a body-less unit/tuple struct, Python's `:` after the parameter list).
    strip_trailing: &'static [char],
}

fn keep_all(_name: &str) -> bool {
    true
}

fn keep_python_public(name: &str) -> bool {
    !name.starts_with('_')
}

fn keep_go_exported(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

fn lang_for_ext(ext: &str) -> Option<LangSpec> {
    Some(match ext {
        "rs" => LangSpec {
            lang_name: "rust",
            language: tree_sitter_rust::LANGUAGE.into(),
            // Only `pub` items are exported; private items are excluded by
            // requiring the `(visibility_modifier)` child in the pattern.
            query_src: r#"
                (function_item (visibility_modifier) name: (identifier) @name) @def
                (struct_item (visibility_modifier) name: (type_identifier) @name) @def
                (enum_item (visibility_modifier) name: (type_identifier) @name) @def
                (trait_item name: (type_identifier) @name) @def
                (use_declaration argument: (_) @import)
            "#,
            name_filter: keep_all,
            strip_trailing: &[';'],
        },
        "py" => LangSpec {
            lang_name: "python",
            language: tree_sitter_python::LANGUAGE.into(),
            // The grammar has no notion of "public"; capture every def/class
            // and drop names starting with `_` in name_filter (PEP 8 convention).
            query_src: r#"
                (function_definition name: (identifier) @name) @def
                (class_definition name: (identifier) @name) @def
                (import_statement name: (dotted_name) @import)
                (import_from_statement module_name: (dotted_name) @import)
            "#,
            name_filter: keep_python_public,
            strip_trailing: &[':'],
        },
        "js" => LangSpec {
            lang_name: "javascript",
            language: tree_sitter_javascript::LANGUAGE.into(),
            // Only symbols wrapped in an `export_statement` are captured at
            // all, so a bare `function helper() {}` never matches.
            query_src: r#"
                (export_statement declaration: (function_declaration name: (identifier) @name)) @def
                (export_statement declaration: (class_declaration name: (identifier) @name)) @def
                (import_statement source: (string) @import)
            "#,
            name_filter: keep_all,
            strip_trailing: &[],
        },
        "ts" => LangSpec {
            lang_name: "typescript",
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            query_src: r#"
                (export_statement declaration: (function_declaration name: (identifier) @name)) @def
                (export_statement declaration: (class_declaration name: (type_identifier) @name)) @def
                (import_statement source: (string) @import)
            "#,
            name_filter: keep_all,
            strip_trailing: &[],
        },
        "go" => LangSpec {
            lang_name: "go",
            language: tree_sitter_go::LANGUAGE.into(),
            // Go has no `export` keyword; "exported" means the identifier's
            // first rune is uppercase. The query can't express that, so it
            // captures every func/type and name_filter drops the unexported ones.
            query_src: r#"
                (function_declaration name: (identifier) @name) @def
                (type_declaration (type_spec name: (type_identifier) @name)) @def
                (import_spec path: (interpreted_string_literal) @import)
            "#,
            name_filter: keep_go_exported,
            strip_trailing: &[],
        },
        _ => return None,
    })
}

impl Extractor for CodeExtractor {
    fn extensions(&self) -> &[&str] {
        &["rs", "py", "js", "ts", "go"]
    }

    fn extract(&self, rel_path: &str, text: &str) -> Entity {
        let ext = rel_path.rsplit('.').next().unwrap_or("");
        let (lang_name, symbols, imports, docstring) = match extract_code(ext, text) {
            Some(v) => v,
            None => (ext.to_string(), Vec::new(), Vec::new(), None),
        };

        let docstring = docstring.or_else(|| leading_doc(text, ext));
        let first_sig = symbols.first().map(String::as_str);
        // `body` is deliberately NOT the raw source text here: source code is
        // not prose, so letting summarize() scan it line-by-line for a "real
        // sentence" produces garbage. Only a real docstring, or (failing
        // that) the first exported signature, is an acceptable summary.
        let summary = summarize(None, docstring.as_deref(), "", first_sig);
        let name = derive_name_from_path(rel_path);

        Entity {
            id: slugify(&name),
            name,
            aliases: Vec::new(),
            created: String::new(),
            body: text.to_string(),
            source_path: String::new(),
            kind: SourceKind::Code { lang: lang_name },
            content_hash: [0u8; 32],
            summary,
            symbols,
            imports,
        }
    }
}

type CodeInfo = (String, Vec<String>, Vec<String>, Option<String>);

fn extract_code(ext: &str, text: &str) -> Option<CodeInfo> {
    let spec = lang_for_ext(ext)?;
    let mut parser = Parser::new();
    parser.set_language(&spec.language).ok()?;
    let tree = parser.parse(text, None)?;
    let query = Query::new(&spec.language, spec.query_src).ok()?;
    let def_idx = query.capture_index_for_name("def");
    let name_idx = query.capture_index_for_name("name");
    let imp_idx = query.capture_index_for_name("import");

    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        let mut def_node: Option<Node> = None;
        let mut name_text: Option<&str> = None;
        for cap in m.captures {
            if Some(cap.index) == def_idx {
                def_node = Some(cap.node);
            } else if Some(cap.index) == name_idx {
                name_text = text.get(cap.node.byte_range());
            } else if Some(cap.index) == imp_idx {
                if let Some(raw) = text.get(cap.node.byte_range()) {
                    imports.push(raw.trim().trim_matches(['"', '\'']).to_string());
                }
            }
        }
        if let Some(def) = def_node {
            let keep = name_text.map(|n| (spec.name_filter)(n)).unwrap_or(true);
            if keep {
                let body = find_body(def);
                let sig = build_signature(text, def, body, spec.strip_trailing);
                if !sig.is_empty() {
                    symbols.push(sig);
                }
            }
        }
    }
    symbols.sort();
    symbols.dedup();
    imports.sort();
    imports.dedup();

    let docstring = if ext == "py" {
        python_docstring(&tree, text)
    } else {
        None
    };

    Some((spec.lang_name.to_string(), symbols, imports, docstring))
}

/// Locate the "body" of a definition node, so its text can be excluded from
/// the extracted signature. Most grammars expose this directly as a `body`
/// field; JS/TS definitions captured through an `export_statement` wrapper
/// expose it one level down, via the wrapped `declaration`'s own `body`.
fn find_body(def: Node) -> Option<Node> {
    if let Some(body) = def.child_by_field_name("body") {
        return Some(body);
    }
    let declaration = def.child_by_field_name("declaration")?;
    declaration.child_by_field_name("body")
}

/// Build a one-line signature from a definition node: its source text up to
/// (not including) the start of its body, with internal whitespace/newlines
/// collapsed to single spaces, trimmed, and language-specific trailing
/// punctuation (e.g. Rust's `;`, Python's `:`) removed.
fn build_signature(text: &str, def: Node, body: Option<Node>, strip_trailing: &[char]) -> String {
    let start = def.start_byte();
    let end = body
        .map(|b| b.start_byte())
        .unwrap_or_else(|| def.end_byte())
        .max(start)
        .min(text.len());
    let raw = text.get(start..end).unwrap_or("");
    let mut sig = collapse_whitespace(raw);
    loop {
        match sig.chars().last() {
            Some(c) if strip_trailing.contains(&c) => {
                sig.pop();
                sig = sig.trim_end().to_string();
            }
            _ => break,
        }
    }
    sig
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.trim().chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

/// A Python module-level docstring: the first non-comment top-level
/// statement, if it is a bare string expression, with its first non-empty
/// inner line returned. Handles both single-line (`"""Doc."""`) and
/// multi-line (`"""\nDoc.\n"""`) docstrings, since it reads the AST's
/// `string_content` node rather than assuming the doc text is on line 1.
fn python_docstring(tree: &Tree, text: &str) -> Option<String> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let first_stmt = root.children(&mut cursor).find(|n| n.kind() != "comment")?;
    if first_stmt.kind() != "expression_statement" {
        return None;
    }
    let mut inner_cursor = first_stmt.walk();
    let string_node = first_stmt
        .children(&mut inner_cursor)
        .find(|n| n.kind() == "string")?;
    let mut content_cursor = string_node.walk();
    let content_node = string_node
        .children(&mut content_cursor)
        .find(|n| n.kind() == "string_content")?;
    let raw = text.get(content_node.byte_range())?;
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Module-level doc comment, one line, if present. This is the fallback for
/// languages without an AST-verified docstring convention (everything but
/// Python, where `python_docstring` is used instead).
fn leading_doc(text: &str, ext: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let doc = match ext {
            "rs" => line
                .strip_prefix("//!")
                .or_else(|| line.strip_prefix("///")),
            "py" => line
                .strip_prefix("\"\"\"")
                .map(|s| s.trim_end_matches("\"\"\"")),
            "js" | "ts" | "go" => line.strip_prefix("//").or_else(|| line.strip_prefix("/*")),
            _ => None,
        };
        return doc.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
    }
    None
}

fn derive_name_from_path(rel_path: &str) -> String {
    let base = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let stem = base.split('.').next().unwrap_or(base);
    stem.replace(['_', '-'], " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_signatures_gated_and_imports() {
        let src = "//! Module docs.\nuse crate::graph::build;\npub fn render_page(x: i32) -> String { String::new() }\nfn private_helper() {}\n";
        let e = CodeExtractor.extract("src/rewrite.rs", src);
        match &e.kind {
            crate::model::SourceKind::Code { lang } => assert_eq!(lang, "rust"),
            _ => panic!("expected Code kind"),
        }
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub fn render_page(x: i32) -> String"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("private_helper")));
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert_eq!(e.summary.as_deref(), Some("Module docs."));
    }

    #[test]
    fn python_signatures_gated_docstring_and_imports() {
        let src = "\"\"\"\nTop docstring.\n\"\"\"\nimport os\nfrom graph import build\ndef extract_all(d):\n    pass\ndef _private():\n    pass\n";
        let e = CodeExtractor.extract("extractor.py", src);
        assert!(
            e.symbols.iter().any(|s| s == "def extract_all(d)"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("_private")));
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert!(e.imports.iter().any(|i| i.contains("os")));
        assert_eq!(e.summary.as_deref(), Some("Top docstring."));
    }

    #[test]
    fn js_export_gated_signatures() {
        let src = "export function foo(a, b) {\n  return a + b;\n}\nfunction helper() {\n  return 1;\n}\n";
        let e = CodeExtractor.extract("mod.js", src);
        assert!(
            e.symbols.iter().any(|s| s == "export function foo(a, b)"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("helper")));
    }

    #[test]
    fn js_import_source_captured() {
        let src = "import { thing } from \"./thing.js\";\nexport function foo() {}\n";
        let e = CodeExtractor.extract("mod.js", src);
        assert!(
            e.imports.iter().any(|i| i.contains("thing.js")),
            "imports: {:?}",
            e.imports
        );
    }

    #[test]
    fn ts_export_gated_signatures() {
        let src = "export class Bar {}\nexport function baz(x: number): void {\n  return;\n}\nfunction hidden() {\n  return 1;\n}\n";
        let e = CodeExtractor.extract("mod.ts", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "export function baz(x: number): void"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "export class Bar"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("hidden")));
    }

    #[test]
    fn go_exported_gated_signatures_and_imports() {
        let src = "package main\n\nimport \"fmt\"\n\nfunc Foo(a int) string {\n\treturn fmt.Sprint(a)\n}\n\nfunc bar() {\n}\n";
        let e = CodeExtractor.extract("main.go", src);
        assert!(
            e.symbols.iter().any(|s| s == "func Foo(a int) string"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("bar")));
        assert!(e.imports.iter().any(|i| i.contains("fmt")));
    }
}
