//! Tests for `extract_simple`.
use crate::formats::code::testutil::{assert_has, assert_lacks, doc_of, item_lines};

#[test]
fn a_triple_slash_directive_is_no_doc() {
    let src = "const a = 1;\n/// <reference path=\"types.d.ts\"/>\nexport function f() {}\n";
    assert_eq!(doc_of("f.ts", src, "f"), (None, 3));
}

#[test]
fn defined_ts_generic_function_and_go_struct_keep_original_case() {
    let ts = "export function createClient<T>(url: string): T {\n  return null as T;\n}\n";
    let e = crate::extract("api.ts", ts);
    assert_eq!(e.defined, vec!["createClient"], "defined: {:?}", e.defined);

    let go = "package s\n\ntype Server struct {}\n\nfunc New() *Server { return nil }\n";
    let e = crate::extract("server.go", go);
    assert_eq!(e.defined, vec!["New", "Server"], "defined: {:?}", e.defined);
}

#[test]
fn js_export_gated_signatures() {
    let src =
        "export function foo(a, b) {\n  return a + b;\n}\nfunction helper() {\n  return 1;\n}\n";
    let e = crate::extract("mod.js", src);
    assert_has(&e.symbols, "export function foo(a, b)");
    assert_lacks(&e.symbols, "helper");
}

#[test]
fn js_import_source_captured() {
    let src = "import { thing } from \"./thing.js\";\nexport function foo() {}\n";
    let e = crate::extract("mod.js", src);
    assert_eq!(e.imports, ["./thing.js"], "{:?}", e.imports);
}

#[test]
fn ts_export_gated_signatures() {
    let src = "export class Bar {}\nexport function baz(x: number): void {\n  return;\n}\nfunction hidden() {\n  return 1;\n}\n";
    let e = crate::extract("mod.ts", src);
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "export function baz(x: number): void"),
        "symbols: {:?}",
        e.symbols
    );
    assert_has(&e.symbols, "export class Bar");
    assert_lacks(&e.symbols, "hidden");
}

#[test]
fn go_exported_gated_signatures_and_imports() {
    let src = "package main\n\nimport \"fmt\"\n\nfunc Foo(a int) string {\n\treturn fmt.Sprint(a)\n}\n\nfunc bar() {\n}\n";
    let e = crate::extract("main.go", src);
    assert_has(&e.symbols, "func Foo(a int) string");
    assert_lacks(&e.symbols, "bar");
    assert_eq!(e.imports, ["fmt"], "{:?}", e.imports);
}

#[test]
fn go_items_cover_methods_and_unexported_functions() {
    let src = "package s\n\n// Server serves.\ntype Server struct{}\n\n// Run starts it.\nfunc (s *Server) Run() {}\n\nfunc (s *Server) stop() {}\n\nfunc helper() {}\n";
    let e = crate::extract("s.go", src);
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
    assert_eq!(e.methods, [] as [String; 0]);
    assert_lacks(&e.symbols, "Run");
}

#[test]
fn a_trailing_comment_on_the_line_above_is_not_a_go_doc() {
    let src = "package p\n\nvar limit = 5 // the limit\nfunc helper() {}\n";
    let e = crate::extract("p.go", src);
    let helper = e.items.iter().find(|i| i.name == "helper").unwrap();
    assert_eq!((helper.start_line, helper.end_line), (4, 4));
    assert_eq!(helper.doc, None);
}

#[test]
fn consecutive_go_doc_lines_join_into_one_doc() {
    let src = "package p\n\n// Run starts the\n// server. Then more.\nfunc Run() {}\n";
    let e = crate::extract("p.go", src);
    assert_eq!(item_lines(&e.items), vec!["3-5 fn Run pub"]);
    assert_eq!(e.items[0].doc.as_deref(), Some("Run starts the server."));
}

#[test]
fn go_directives_stay_in_the_range_but_out_of_the_doc() {
    let src = "package p\n\n// Gen makes things.\n//go:generate stringer -type=X\n//nolint\nfunc Gen() {}\n\n//go:noinline\n//lint:ignore U1000 kept\nfunc fast() {}\n\n//export Hook\n//line hook.go:1\nfunc Hook() {}\n";
    let e = crate::extract("p.go", src);
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
    let e = crate::extract("p.go", src);
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
    let e = crate::extract("m.ts", src);
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
    let e = crate::extract("s.go", src);
    assert_eq!(
        item_lines(&e.items),
        vec!["3-3 method Box::A pub", "5-5 method Box::B pub"]
    );
}

#[test]
fn ts_items_cover_private_functions_classes_methods_and_arrows() {
    let src = "/** Adds two. */\nexport function add(a: number, b: number): number {\n  function inner() {}\n  return a + b;\n}\n\nfunction hidden() {}\n\nexport class Box {\n  /** Opens it. */\n  open(): void {}\n}\n\nconst twice = (x: number) => x * 2;\n\ninterface Shape {}\n";
    let e = crate::extract("m.ts", src);
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
        let e = crate::extract("m.ts", src);
        let item = e.items.iter().find(|i| i.name == name).unwrap();
        (item.doc.clone(), item.start_line)
    };
    let src = "import x from 'y';\n// Thrown for any non-2xx response.\nexport class ApiError {}\n";
    assert_eq!(
        doc_of(src, "ApiError"),
        (Some("Thrown for any non-2xx response.".into()), 2)
    );
    let src = "import x from 'y';\n/* Parses the reply\n * body as JSON. */\nfunction parse() {}\n";
    assert_eq!(
        doc_of(src, "parse"),
        (Some("Parses the reply body as JSON.".into()), 2)
    );
    let src = "import x from 'y';\n// Compares with === so NaN never matches.\nfunction eq() {}\n";
    assert_eq!(
        doc_of(src, "eq"),
        (Some("Compares with === so NaN never matches.".into()), 2)
    );
    let src = "// plain\n/** Real doc. */\nfunction f() {}\n";
    assert_eq!(doc_of(src, "f"), (Some("Real doc.".into()), 2));
}
