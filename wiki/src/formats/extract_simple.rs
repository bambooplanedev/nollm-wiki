//! JS, TS, and Go: languages whose export surface is expressed by the grammar
//! (`export_statement`) or by a naming convention (Go's leading capital), with
//! no owner resolution and no scope guard.
use super::code::{
    always_free, keep_all, keep_any_vis, no_body_doc, no_export_set, no_header_group, no_lead,
    owner_anchor_identity, sig_start_identity, LangSpec, Lead, Placement,
};
use tree_sitter::{Language, Node};

fn keep_go_exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

/// The JS and TS specs differ only in grammar and in the node kind a class
/// name carries; everything else is shared here.
fn ecma_spec(lang_name: &'static str, language: Language, query_src: &'static str) -> LangSpec {
    LangSpec {
        lang_name,
        language,
        query_src,
        name_filter: keep_all,
        vis_filter: keep_any_vis,
        strip_trailing: &[],
        placement: always_free,
        owner_sep: "",
        export_set: no_export_set,
        join_continuations: false,
        sig_start: sig_start_identity,
        owner_anchor: owner_anchor_identity,
        header_group: no_header_group,
        item_lead: no_lead,
        body_doc: no_body_doc,
        query_gates_visibility: false,
    }
}

pub(crate) fn js_spec() -> LangSpec {
    ecma_spec(
        "javascript",
        tree_sitter_javascript::LANGUAGE.into(),
        // Only symbols wrapped in an `export_statement` are captured at
        // all, so a bare `function helper() {}` never matches.
        r"
                (export_statement declaration: (function_declaration name: (identifier) @name)) @def
                (export_statement declaration: (class_declaration name: (identifier) @name)) @def
                (import_statement source: (string) @import)
            ",
    )
}

pub(crate) fn ts_spec() -> LangSpec {
    ecma_spec(
        "typescript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        r"
                (export_statement declaration: (function_declaration name: (identifier) @name)) @def
                (export_statement declaration: (class_declaration name: (type_identifier) @name)) @def
                (import_statement source: (string) @import)
            ",
    )
}

pub(crate) fn go_spec() -> LangSpec {
    LangSpec {
        lang_name: "go",
        language: tree_sitter_go::LANGUAGE.into(),
        // Go has no `export` keyword; "exported" means the identifier's
        // first rune is uppercase. The query can't express that, so it
        // captures every func/type and name_filter drops the unexported ones.
        query_src: r"
                (function_declaration name: (identifier) @name) @def
                (type_declaration (type_spec name: (type_identifier) @name)) @def
                (method_declaration name: (field_identifier) @name) @item
                (import_spec path: (interpreted_string_literal) @import)
            ",
        name_filter: keep_go_exported,
        vis_filter: keep_any_vis,
        strip_trailing: &[],
        placement: go_placement,
        owner_sep: "",
        export_set: no_export_set,
        join_continuations: false,
        sig_start: sig_start_identity,
        owner_anchor: owner_anchor_identity,
        header_group: no_header_group,
        item_lead: go_lead,
        body_doc: no_body_doc,
        query_gates_visibility: false,
    }
}

/// A method belongs to its receiver's type: `*Server`, `Server` and
/// `*Server[T]` all give `Server`. Everything else is a free item.
fn go_placement(def: Node, text: &str) -> Placement {
    let receiver = (def.kind() == "method_declaration")
        .then(|| def.child_by_field_name("receiver"))
        .flatten()
        .and_then(|r| r.named_child(0))
        .and_then(|p| p.child_by_field_name("type"))
        .and_then(|t| text.get(t.byte_range()));
    Placement::Scoped(
        receiver
            .map(|t| {
                let t = t.trim_start_matches('*');
                t.split('[').next().unwrap_or(t).trim().to_string()
            })
            .into_iter()
            .collect(),
    )
}

/// Go's doc convention is the `//` comment lines directly above a declaration.
fn go_lead(node: Node, text: &str) -> Lead {
    match text
        .get(node.byte_range())
        .and_then(|t| t.strip_prefix("//"))
    {
        Some(doc) if node.kind() == "comment" => Lead::Doc(doc.trim().to_string()),
        _ => Lead::Stop,
    }
}

#[cfg(test)]
mod tests {
    use crate::formats::code::testutil::item_lines;
    use crate::formats::code::CodeExtractor;
    use crate::formats::Extractor;

    #[test]
    fn defined_ts_generic_function_and_go_struct_keep_original_case() {
        let ts = "export function createClient<T>(url: string): T {\n  return null as T;\n}\n";
        let e = CodeExtractor.extract("api.ts", ts);
        assert_eq!(e.defined, vec!["createClient"], "defined: {:?}", e.defined);

        let go = "package s\n\ntype Server struct {}\n\nfunc New() *Server { return nil }\n";
        let e = CodeExtractor.extract("server.go", go);
        assert_eq!(e.defined, vec!["New", "Server"], "defined: {:?}", e.defined);
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

    #[test]
    fn go_items_cover_methods_and_unexported_functions() {
        let src = "package s\n\n// Server serves.\ntype Server struct{}\n\n// Run starts it.\nfunc (s *Server) Run() {}\n\nfunc (s *Server) stop() {}\n\nfunc helper() {}\n";
        let e = CodeExtractor.extract("s.go", src);
        assert_eq!(
            item_lines(&e.items),
            vec![
                "3-4 type Server pub",
                "6-7 method Server::Run pub",
                "9-9 method Server::stop priv",
                "11-11 fn helper priv",
            ]
        );
        assert_eq!(e.items[1].doc.as_deref(), Some("Run starts it."));
        assert!(e.methods.is_empty());
        assert!(!e.symbols.iter().any(|s| s.contains("Run")));
    }

    #[test]
    fn go_receiver_types_lose_pointer_and_type_arguments() {
        let src = "package s\n\nfunc (b Box) A() {}\n\nfunc (b *Box[T]) B() {}\n";
        let e = CodeExtractor.extract("s.go", src);
        assert_eq!(
            item_lines(&e.items),
            vec!["3-3 method Box::A pub", "5-5 method Box::B pub"]
        );
    }
}
