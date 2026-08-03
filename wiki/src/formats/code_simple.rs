//! JS, TS, and Go: languages whose export surface is expressed by the grammar
//! (`export_statement`) or by a naming convention (Go's leading capital), with
//! no owner resolution and no scope guard.
use super::code::{any_def, keep_all, keep_any_vis, no_owner, LangSpec};

fn keep_go_exported(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

pub(crate) fn js_spec() -> LangSpec {
    LangSpec {
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
        vis_filter: keep_any_vis,
        strip_trailing: &[],
        owner_of: no_owner,
        owner_sep: "",
        def_filter: any_def,
    }
}

pub(crate) fn ts_spec() -> LangSpec {
    LangSpec {
        lang_name: "typescript",
        language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        query_src: r#"
                (export_statement declaration: (function_declaration name: (identifier) @name)) @def
                (export_statement declaration: (class_declaration name: (type_identifier) @name)) @def
                (import_statement source: (string) @import)
            "#,
        name_filter: keep_all,
        vis_filter: keep_any_vis,
        strip_trailing: &[],
        owner_of: no_owner,
        owner_sep: "",
        def_filter: any_def,
    }
}

pub(crate) fn go_spec() -> LangSpec {
    LangSpec {
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
        vis_filter: keep_any_vis,
        strip_trailing: &[],
        owner_of: no_owner,
        owner_sep: "",
        def_filter: any_def,
    }
}

#[cfg(test)]
mod tests {
    use crate::formats::code::CodeExtractor;
    use crate::formats::Extractor;

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
