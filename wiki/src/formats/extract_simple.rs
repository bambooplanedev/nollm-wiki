//! JS, TS, and Go: languages whose export surface is expressed by the grammar
//! (`export_statement`) or by a naming convention (Go's leading capital), with
//! no owner resolution and no scope guard.
use super::code::{
    default_shape, is_function, keep_all, keep_any_vis, no_body_doc, no_export_set,
    no_header_group, owner_anchor_identity, plain_comment_text, LangSpec, Lead, Placement, Rank,
    Shape,
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
        // `const twice = …` is cut at its value, which leaves the `=`.
        strip_trailing: &['='],
        placement: ecma_placement,
        owner_sep: ".",
        export_set: no_export_set,
        join_continuations: false,
        sig_start: ecma_sig_start,
        owner_anchor: owner_anchor_identity,
        header_group: no_header_group,
        item_lead: ecma_lead,
        body_doc: no_body_doc,
        // The query has no per-member visibility, so a method of an exported
        // class reads private: a known gap. Only an `export` @def is public.
        query_gates_visibility: true,
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
                (function_declaration name: (identifier) @name) @item
                (generator_function_declaration name: (identifier) @name) @item
                (method_definition name: [(property_identifier) (private_property_identifier)] @name) @item
                (variable_declarator name: (identifier) @name) @item
                (class_declaration name: (identifier) @name) @item
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
                (function_declaration name: (identifier) @name) @item
                (generator_function_declaration name: (identifier) @name) @item
                (method_definition name: [(property_identifier) (private_property_identifier)] @name) @item
                (variable_declarator name: (identifier) @name) @item
                (class_declaration name: (type_identifier) @name) @item
                (abstract_class_declaration name: (type_identifier) @name) @item
                (interface_declaration name: (type_identifier) @name) @item
                (type_alias_declaration name: (type_identifier) @name) @item
                (enum_declaration name: (identifier) @name) @item
                (import_statement source: (string) @import)
            ",
    )
}

/// Functions and methods hold no items of their own (a nested helper is not
/// part of the file's surface), while a class names its methods' owner.
fn ecma_placement(def: Node, text: &str) -> Placement {
    let mut chain = Vec::new();
    let mut up = def.parent();
    while let Some(n) = up {
        match n.kind() {
            "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "function"
            | "arrow_function"
            | "method_definition"
            | "generator_function" => return Placement::Rejected,
            "class_declaration" | "class" | "abstract_class_declaration" => {
                if let Some(name) = n
                    .child_by_field_name("name")
                    .and_then(|n| text.get(n.byte_range()))
                {
                    chain.push(name.to_string());
                }
            }
            _ => {}
        }
        up = n.parent();
    }
    chain.reverse();
    Placement::Scoped(chain)
}

/// A variable whose value is not a function is a value. It keeps the shared
/// cut at its value, so `const LIMIT = 5` reads `const LIMIT`.
pub(crate) fn ecma_shape(def: Node) -> Shape {
    let rank = if def.kind() == "variable_declarator" && !is_function(def) {
        Rank::Value
    } else {
        Rank::Def
    };
    Shape {
        rank,
        ..default_shape(def)
    }
}

/// A declaration's range and signature start at its `export` or `const`.
fn ecma_sig_start(def: Node) -> Node {
    let mut top = def;
    while let Some(p) = top.parent().filter(|p| {
        matches!(
            p.kind(),
            "lexical_declaration" | "variable_declaration" | "export_statement"
        )
    }) {
        top = p;
    }
    top
}

/// A `JSDoc` block (`/** … */`) above a declaration is its doc, and any other
/// comment a plain lead.
fn ecma_lead(node: Node, text: &str) -> Lead {
    match text
        .get(node.byte_range())
        .filter(|_| node.kind() == "comment")
    {
        Some(t) if t.starts_with("/**") && !t.starts_with("/***") && !t.starts_with("/**/") => {
            let body = t.trim_start_matches("/**").trim_end_matches("*/");
            let lines: Vec<&str> = body
                .lines()
                .map(|l| l.trim().trim_start_matches('*').trim())
                .filter(|l| !l.is_empty())
                .collect();
            Lead::Doc(lines.join("\n"))
        }
        // A triple-slash directive (`/// <reference …/>`) is no doc.
        Some(t) if t.starts_with("/// <") => Lead::Stop,
        Some(t) => Lead::Plain(plain_comment_text(t)),
        None => Lead::Stop,
    }
}

pub(crate) fn go_spec() -> LangSpec {
    LangSpec {
        lang_name: "go",
        language: tree_sitter_go::LANGUAGE.into(),
        // Go has no `export` keyword; "exported" means the identifier's
        // first rune is uppercase. The query can't express that, so it
        // captures every func/type and name_filter drops the unexported ones.
        // A spec's names are matched without their `name` field: the grammar
        // puts the field on the commas of `const B, C = 2, 3` too, so the
        // field form never reaches `C`. A spec's only direct `identifier`
        // children are its names (a type is a `type_identifier`).
        query_src: r"
                (function_declaration name: (identifier) @name) @def
                (type_declaration (type_spec name: (type_identifier) @name)) @def
                (method_declaration name: (field_identifier) @name) @item
                (const_spec (identifier) @name) @item
                (var_spec (identifier) @name) @item
                (import_spec path: (interpreted_string_literal) @import)
            ",
        name_filter: keep_go_exported,
        vis_filter: keep_any_vis,
        strip_trailing: &[],
        placement: go_placement,
        owner_sep: "",
        export_set: no_export_set,
        join_continuations: false,
        sig_start: go_sig_start,
        owner_anchor: owner_anchor_identity,
        header_group: no_header_group,
        item_lead: go_lead,
        body_doc: no_body_doc,
        query_gates_visibility: false,
    }
}

/// A method belongs to its receiver's type: `*Server`, `Server` and
/// `*Server[T]` all give `Server`. Everything else is a free item, except a
/// `const` or `var` inside a function, which is no item at all.
fn go_placement(def: Node, text: &str) -> Placement {
    let local = matches!(def.kind(), "const_spec" | "var_spec")
        && std::iter::successors(def.parent(), Node::parent).any(|n| {
            matches!(
                n.kind(),
                "function_declaration" | "method_declaration" | "func_literal"
            )
        });
    if local {
        return Placement::Rejected;
    }
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

/// An ungrouped `const X = 1` starts at its `const`, where its doc comment
/// sits; a spec inside `const ( … )` or `var ( … )` starts at itself, so
/// each spec keeps its own lines and doc.
fn go_sig_start(def: Node) -> Node {
    match def.parent() {
        Some(p)
            if matches!(p.kind(), "const_declaration" | "var_declaration")
                && !p.children(&mut p.walk()).any(|c| c.kind() == "(") =>
        {
            p
        }
        _ => def,
    }
}

/// A `const`/`var` spec is a value: it stops at its `=` and re-appends the
/// value within `VALUE_BUDGET`. Everything else is the shared default.
pub(crate) fn go_shape(def: Node) -> Shape {
    if !matches!(def.kind(), "const_spec" | "var_spec") {
        return default_shape(def);
    }
    let eq = def.children(&mut def.walk()).find(|c| c.kind() == "=");
    let value = def.child_by_field_name("value");
    Shape {
        rank: Rank::Value,
        cut: eq.or(value),
        value: eq.and(value),
        has_type: def.child_by_field_name("type").is_some(),
    }
}

/// Go's doc convention is the `//` comment lines directly above a declaration.
/// A directive (`//go:generate`, `//nolint`, `//export F`) sits in the same
/// place but instructs a tool, so it belongs to the range and not the doc.
fn go_lead(node: Node, text: &str) -> Lead {
    match text
        .get(node.byte_range())
        .and_then(|t| t.strip_prefix("//"))
    {
        Some(doc) if node.kind() == "comment" && is_go_directive(doc) => Lead::Attr,
        Some(doc) if node.kind() == "comment" => Lead::Doc(doc.trim().to_string()),
        _ => Lead::Stop,
    }
}

/// A `//` comment body that is a directive: the named forms, or the general
/// `//tool:args` shape, which has no space after `//`.
fn is_go_directive(body: &str) -> bool {
    let general = body.split_once(':').is_some_and(|(tool, rest)| {
        let lower_alnum = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
        !tool.is_empty() && tool.chars().all(lower_alnum) && rest.starts_with(lower_alnum)
    });
    general
        || ["go:", "line ", "export ", "extern ", "nolint"]
            .iter()
            .any(|p| body.starts_with(p))
}

#[cfg(test)]
mod tests {
    use crate::formats::code::testutil::{assert_lacks, item_lines};
    use crate::formats::code::CodeExtractor;
    use crate::formats::Extractor;

    /// The `(doc, start_line)` of the item named `name` in `src`.
    fn doc_of(path: &str, src: &str, name: &str) -> (Option<String>, usize) {
        let e = CodeExtractor.extract(path, src);
        let item = e.items.iter().find(|i| i.name == name).unwrap();
        (item.doc.clone(), item.start_line)
    }

    #[test]
    fn a_jsdoc_keeps_a_plain_directive_below_it_out_of_the_doc() {
        let src = "/** Best bid. */\n// eslint-disable-next-line no-unused-vars because legacy API\nexport function getBids() {}\n";
        assert_eq!(doc_of("b.ts", src, "getBids"), (None, 3));
    }

    #[test]
    fn a_triple_slash_directive_is_no_doc() {
        let src = "const a = 1;\n/// <reference path=\"types.d.ts\"/>\nexport function f() {}\n";
        assert_eq!(doc_of("f.ts", src, "f"), (None, 3));
    }

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
    fn a_trailing_comment_on_the_line_above_is_not_a_go_doc() {
        let src = "package p\n\nvar limit = 5 // the limit\nfunc helper() {}\n";
        let e = CodeExtractor.extract("p.go", src);
        let helper = e.items.iter().find(|i| i.name == "helper").unwrap();
        assert_eq!((helper.start_line, helper.end_line), (4, 4));
        assert_eq!(helper.doc, None);
    }

    #[test]
    fn consecutive_go_doc_lines_join_into_one_doc() {
        let src = "package p\n\n// Run starts the\n// server. Then more.\nfunc Run() {}\n";
        let e = CodeExtractor.extract("p.go", src);
        assert_eq!(item_lines(&e.items), vec!["3-5 fn Run pub"]);
        assert_eq!(e.items[0].doc.as_deref(), Some("Run starts the server."));
    }

    #[test]
    fn go_directives_stay_in_the_range_but_out_of_the_doc() {
        let src = "package p\n\n// Gen makes things.\n//go:generate stringer -type=X\n//nolint\nfunc Gen() {}\n\n//go:noinline\n//lint:ignore U1000 kept\nfunc fast() {}\n\n//export Hook\n//line hook.go:1\nfunc Hook() {}\n";
        let e = CodeExtractor.extract("p.go", src);
        assert_eq!(
            item_lines(&e.items),
            vec!["3-6 fn Gen pub", "8-10 fn fast priv", "12-14 fn Hook pub"]
        );
        assert_eq!(e.items[0].doc.as_deref(), Some("Gen makes things."));
        assert_eq!(e.items[1].doc, None);
        assert_eq!(e.items[2].doc, None);
    }

    #[test]
    fn go_package_level_consts_and_vars_are_items_one_per_name() {
        let src = "package p\n\n// L is the limit.\nconst L = 5\nvar limit = 5 // the limit\nconst (\n\t// A doc.\n\tA = 1\n\tB, C int = 2, 3\n)\nvar (\n\tx int\n\ty = 2\n)\nfunc f() { const z = 1; var w = 2 }\n";
        let e = CodeExtractor.extract("p.go", src);
        assert_eq!(
            item_lines(&e.items),
            vec![
                "3-4 value L pub",
                "5-5 value limit priv",
                "7-8 value A pub",
                "9-9 value B pub",
                "9-9 value C pub",
                "12-12 value x priv",
                "13-13 value y priv",
                "15-15 fn f priv",
            ]
        );
        let sigs: Vec<&str> = e.items.iter().map(|i| i.signature.as_str()).collect();
        assert_eq!(
            sigs[..7],
            [
                "const L = 5",
                "var limit = 5",
                "A = 1",
                "B, C int = 2, 3",
                "B, C int = 2, 3",
                "x int",
                "y = 2"
            ]
        );
        assert_eq!(e.items[0].doc.as_deref(), Some("L is the limit."));
        assert_eq!(e.items[1].doc, None);
        assert_eq!(e.items[2].doc.as_deref(), Some("A doc."));
        assert!(
            e.symbols.is_empty() && e.defined.is_empty(),
            "{:?}",
            e.symbols
        );
    }

    #[test]
    fn ts_top_level_variables_are_value_items() {
        let src = "const LIMIT = 5;\nlet counter;\n/** The name. */\nexport const NAME: string = \"x\";\nconst twice = (x: number) => x * 2;\nfunction f() {\n  const local = 1;\n}\n";
        let e = CodeExtractor.extract("m.ts", src);
        assert_eq!(
            item_lines(&e.items),
            vec![
                "1-1 value LIMIT priv",
                "2-2 value counter priv",
                "3-4 value NAME priv",
                "5-5 fn twice priv",
                "6-8 fn f priv",
            ]
        );
        let sigs: Vec<&str> = e.items.iter().map(|i| i.signature.as_str()).collect();
        assert_eq!(
            sigs[..3],
            ["const LIMIT", "let counter", "export const NAME: string"]
        );
        assert_eq!(e.items[2].doc.as_deref(), Some("The name."));
        assert!(e.symbols.is_empty(), "{:?}", e.symbols);
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

    #[test]
    fn ts_items_cover_private_functions_classes_methods_and_arrows() {
        let src = "/** Adds two. */\nexport function add(a: number, b: number): number {\n  function inner() {}\n  return a + b;\n}\n\nfunction hidden() {}\n\nexport class Box {\n  /** Opens it. */\n  open(): void {}\n}\n\nconst twice = (x: number) => x * 2;\n\ninterface Shape {}\n";
        let e = CodeExtractor.extract("m.ts", src);
        assert_eq!(
            item_lines(&e.items),
            vec![
                "1-5 fn add pub",
                "7-7 fn hidden priv",
                "9-12 type Box pub",
                "10-11 method Box::open priv",
                "14-14 fn twice priv",
                "16-16 type Shape priv",
            ]
        );
        assert_eq!(e.items[0].doc.as_deref(), Some("Adds two."));
        assert_eq!(e.items[3].doc.as_deref(), Some("Opens it."));
        assert_eq!(e.items[4].signature, "const twice");
        assert_lacks(&e.symbols, "hidden");
        assert_lacks(&e.symbols, "twice");
    }

    #[test]
    fn ts_line_and_block_comments_are_docs_jsdoc_wins() {
        let doc_of = |src: &str, name: &str| {
            let e = CodeExtractor.extract("m.ts", src);
            let item = e.items.iter().find(|i| i.name == name).unwrap();
            (item.doc.clone(), item.start_line)
        };
        let src =
            "import x from 'y';\n// Thrown for any non-2xx response.\nexport class ApiError {}\n";
        assert_eq!(
            doc_of(src, "ApiError"),
            (Some("Thrown for any non-2xx response.".into()), 2)
        );
        let src =
            "import x from 'y';\n/* Parses the reply\n * body as JSON. */\nfunction parse() {}\n";
        assert_eq!(
            doc_of(src, "parse"),
            (Some("Parses the reply body as JSON.".into()), 2)
        );
        let src =
            "import x from 'y';\n// Compares with === so NaN never matches.\nfunction eq() {}\n";
        assert_eq!(
            doc_of(src, "eq"),
            (Some("Compares with === so NaN never matches.".into()), 2)
        );
        let src = "// plain\n/** Real doc. */\nfunction f() {}\n";
        assert_eq!(doc_of(src, "f"), (Some("Real doc.".into()), 2));
    }
}
