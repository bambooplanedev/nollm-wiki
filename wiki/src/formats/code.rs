use crate::formats::{summarize, Extractor};
use crate::model::{slugify, Entity, SourceKind};
use tree_sitter::{Language, Parser, Query, QueryCursor};

pub struct CodeExtractor;

struct LangSpec {
    lang_name: &'static str,
    language: Language,
    /// Query capturing @symbol (a definition node whose text we shorten to a signature)
    /// and @import (a module path node).
    query_src: &'static str,
}

fn lang_for_ext(ext: &str) -> Option<LangSpec> {
    Some(match ext {
        "rs" => LangSpec {
            lang_name: "rust",
            language: tree_sitter_rust::LANGUAGE.into(),
            query_src: r#"
                (function_item (visibility_modifier) name: (identifier) @symbol)
                (struct_item (visibility_modifier) name: (type_identifier) @symbol)
                (enum_item (visibility_modifier) name: (type_identifier) @symbol)
                (trait_item name: (type_identifier) @symbol)
                (use_declaration argument: (_) @import)
            "#,
        },
        "py" => LangSpec {
            lang_name: "python",
            language: tree_sitter_python::LANGUAGE.into(),
            query_src: r#"
                (function_definition name: (identifier) @symbol)
                (class_definition name: (identifier) @symbol)
                (import_statement name: (dotted_name) @import)
                (import_from_statement module_name: (dotted_name) @import)
            "#,
        },
        "js" => LangSpec {
            lang_name: "javascript",
            language: tree_sitter_javascript::LANGUAGE.into(),
            query_src: r#"
                (function_declaration name: (identifier) @symbol)
                (class_declaration name: (identifier) @symbol)
                (import_statement source: (string) @import)
            "#,
        },
        "ts" => LangSpec {
            lang_name: "typescript",
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            query_src: r#"
                (function_declaration name: (identifier) @symbol)
                (class_declaration name: (type_identifier) @symbol)
                (import_statement source: (string) @import)
            "#,
        },
        "go" => LangSpec {
            lang_name: "go",
            language: tree_sitter_go::LANGUAGE.into(),
            query_src: r#"
                (function_declaration name: (identifier) @symbol)
                (type_declaration (type_spec name: (type_identifier) @symbol))
                (import_spec path: (interpreted_string_literal) @import)
            "#,
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
        let (lang_name, symbols, imports) = match extract_code(ext, text) {
            Some(v) => v,
            None => (ext.to_string(), Vec::new(), Vec::new()),
        };

        let docstring = leading_doc(text, ext);
        let first_sig = symbols.first().map(String::as_str);
        let summary = summarize(None, docstring.as_deref(), text, first_sig);
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

fn extract_code(ext: &str, text: &str) -> Option<(String, Vec<String>, Vec<String>)> {
    let spec = lang_for_ext(ext)?;
    let mut parser = Parser::new();
    parser.set_language(&spec.language).ok()?;
    let tree = parser.parse(text, None)?;
    let query = Query::new(&spec.language, spec.query_src).ok()?;
    let sym_idx = query.capture_index_for_name("symbol");
    let imp_idx = query.capture_index_for_name("import");

    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let node_text = &text[cap.node.byte_range()];
            if Some(cap.index) == sym_idx {
                symbols.push(node_text.trim().to_string());
            } else if Some(cap.index) == imp_idx {
                imports.push(node_text.trim().trim_matches(['"', '\'']).to_string());
            }
        }
    }
    symbols.sort();
    symbols.dedup();
    imports.sort();
    imports.dedup();
    Some((spec.lang_name.to_string(), symbols, imports))
}

/// Module-level doc comment / docstring, one line, if present.
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
    fn rust_exports_and_imports() {
        let src = "//! Module docs.\nuse crate::graph::build;\npub fn render_page(x: i32) -> String { String::new() }\nfn private_helper() {}\n";
        let e = CodeExtractor.extract("src/rewrite.rs", src);
        match &e.kind {
            crate::model::SourceKind::Code { lang } => assert_eq!(lang, "rust"),
            _ => panic!(),
        }
        assert!(e.symbols.iter().any(|s| s.contains("render_page")));
        assert!(!e.symbols.iter().any(|s| s.contains("private_helper")));
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert_eq!(e.summary.as_deref(), Some("Module docs."));
    }

    #[test]
    fn python_defs_and_imports() {
        let src = "\"\"\"Top docstring.\"\"\"\nimport os\nfrom graph import build\ndef extract_all(d):\n    pass\n";
        let e = CodeExtractor.extract("extractor.py", src);
        assert!(e.symbols.iter().any(|s| s.contains("extract_all")));
        assert!(e
            .imports
            .iter()
            .any(|i| i.contains("graph") || i.contains("os")));
        assert_eq!(e.summary.as_deref(), Some("Top docstring."));
    }
}
