use std::fs;

fn compile_tree(files: &[(&str, &str)]) -> serde_json::Value {
    compile_with_items(files).0
}

/// The compiled `index.json` and its `items.json` sidecar.
fn compile_with_items(files: &[(&str, &str)]) -> (serde_json::Value, serde_json::Value) {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    for (path, body) in files {
        let p = src.path().join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    wiki::compile(src.path(), out.path(), &wiki::CompileOptions::default()).unwrap();
    let read = |name: &str| {
        serde_json::from_str(&fs::read_to_string(out.path().join(name)).unwrap()).unwrap()
    };
    (read("index.json"), read("items.json"))
}

fn neighbors_out<'a>(index: &'a serde_json::Value, id: &str) -> Vec<&'a str> {
    index["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap_or_else(|| panic!("no page {id} in {index:#}"))["neighbors_out"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect()
}

#[test]
fn header_and_impl_both_exist_and_the_include_draws_one_edge() {
    let idx = compile_tree(&[
        (
            "core/book/orderbook.h",
            "class OrderBook {\npublic:\n  void apply();\n};\n",
        ),
        (
            "core/book/orderbook.cpp",
            "#include \"core/book/orderbook.h\"\nvoid OrderBook::apply() {}\n",
        ),
        ("core/core.h", "class Core {};\n"),
        ("core/book/book.h", "class Book {};\n"),
    ]);
    let out = neighbors_out(&idx, "orderbook_cpp");
    assert!(out.contains(&"orderbook"), "include edge missing: {out:?}");
    assert!(
        !out.contains(&"core"),
        "path segment became an edge: {out:?}"
    );
    assert!(
        !out.contains(&"book"),
        "path segment became an edge: {out:?}"
    );
}

#[test]
fn third_party_angle_includes_draw_no_edges() {
    let idx = compile_tree(&[
        (
            "src/router.cpp",
            "#include <boost/algorithm/string.hpp>\nvoid go() {}\n",
        ),
        ("src/algorithm.h", "class Algorithm {};\n"),
        ("src/string.h", "class String {};\n"),
    ]);
    let out = neighbors_out(&idx, "router");
    assert!(
        !out.contains(&"algorithm"),
        "boost invented an edge: {out:?}"
    );
    assert!(!out.contains(&"string"), "boost invented an edge: {out:?}");
}

#[test]
fn project_local_angle_include_still_resolves() {
    let idx = compile_tree(&[
        ("include/mylib/core.h", "class Core {};\n"),
        (
            "src/main.cpp",
            "#include <mylib/core.h>\nint main() { return 0; }\n",
        ),
    ]);
    let out = neighbors_out(&idx, "main");
    assert!(
        out.contains(&"core"),
        "own header via -I not resolved: {out:?}"
    );
}

#[test]
fn a_free_function_declared_and_defined_still_resolves_by_name() {
    let idx = compile_tree(&[
        ("src/tick.h", "void tick(int c);\n"),
        ("src/tick.cpp", "void tick(int c) { (void)c; }\n"),
        (
            "src/caller.cpp",
            "#include \"src/tick.h\"\nvoid go() { tick(1); }\n",
        ),
    ]);
    // The header keeps the bare slug; the impl takes the discriminator.
    let ids: Vec<&str> = idx["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"tick"), "ids: {ids:?}");
    assert!(ids.contains(&"tick_cpp"), "ids: {ids:?}");
    let out = neighbors_out(&idx, "caller");
    assert!(
        out.contains(&"tick"),
        "by_defined went dead for a free function: {out:?}"
    );
}

fn items_of(
    (index, items): &(serde_json::Value, serde_json::Value),
    path: &str,
) -> Vec<serde_json::Value> {
    let id = index["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == path)
        .unwrap_or_else(|| panic!("no page {path} in {index:#}"))["id"]
        .as_str()
        .unwrap();
    items[id]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("no items for {path}"))
}

#[test]
fn a_definition_inherits_the_declaration_doc_and_both_point_at_each_other() {
    let idx = compile_with_items(&[
        (
            "include/book.h",
            "class Book {\npublic:\n  /// Applies an order. Drops a stale one.\n  void apply(int o);\n};\n",
        ),
        (
            "src/book.cpp",
            "#include \"book.h\"\nvoid Book::apply(int o) {\n  (void)o;\n}\n",
        ),
    ]);
    let h = items_of(&idx, "include/book.h");
    let apply_h = h.iter().find(|i| i["name"] == "apply").unwrap();
    assert_eq!(apply_h["twin"], "src/book.cpp:2-4");
    let c = items_of(&idx, "src/book.cpp");
    let apply_c = c.iter().find(|i| i["name"] == "apply").unwrap();
    assert_eq!(apply_c["owner"], "Book");
    assert_eq!(apply_c["doc"], "Applies an order.");
    assert_eq!(apply_c["doc_text"], "Applies an order. Drops a stale one.");
    assert_eq!(apply_c["twin"], "include/book.h:3-4");
}

#[test]
fn overloads_pair_by_parameter_count() {
    let idx = compile_with_items(&[
        (
            "book.h",
            "class Book {\npublic:\n  /// One.\n  void apply(int o);\n  /// Two.\n  void apply(int o, std::map<int, int> m);\n};\n",
        ),
        (
            "book.cpp",
            "void Book::apply(int o, std::map<int, int> m) {}\nvoid Book::apply(int o) {}\n",
        ),
    ]);
    let c = items_of(&idx, "book.cpp");
    let doc_at = |line: u64| c.iter().find(|i| i["start_line"] == line).unwrap()["doc"].clone();
    assert_eq!(doc_at(1), "Two.");
    assert_eq!(doc_at(2), "One.");
}

#[test]
fn overloads_are_left_unpaired() {
    let idx = compile_with_items(&[
        ("book.h", "class Book {\npublic:\n  /// Int.\n  void apply(int o);\n  void apply(double o);\n};\n"),
        ("book.cpp", "void Book::apply(int o) {}\nvoid Book::apply(double o) {}\n"),
    ]);
    for path in ["book.h", "book.cpp"] {
        let items = items_of(&idx, path);
        assert_eq!(
            items.iter().filter(|i| i["name"] == "apply").count(),
            2,
            "{path}"
        );
        assert!(items.iter().all(|i| i.get("twin").is_none()), "{path}");
    }
    assert!(items_of(&idx, "book.cpp")
        .iter()
        .all(|i| i.get("doc").is_none()));
}

#[test]
fn same_stem_files_in_several_directories_pair_within_their_directory() {
    let idx = compile_with_items(&[
        ("a/book.h", "/// Doc.\nvoid run();\n"),
        ("a/book.cpp", "void run() {}\n"),
        ("b/book.cpp", "void run() {}\n"),
    ]);
    let run = |path: &str| {
        items_of(&idx, path)
            .into_iter()
            .find(|i| i["name"] == "run")
            .unwrap_or_else(|| panic!("no run in {path}"))
    };
    assert_eq!(run("a/book.cpp")["twin"], "a/book.h:1-2");
    assert_eq!(run("a/book.cpp")["doc"], "Doc.");
    assert!(run("b/book.cpp").get("twin").is_none());
    assert!(run("b/book.cpp").get("doc").is_none());
}

fn sig_of(items: &[serde_json::Value], owner: Option<&str>, name: &str) -> String {
    let it = items
        .iter()
        .find(|i| i["name"] == name && i.get("owner").and_then(|o| o.as_str()) == owner)
        .unwrap_or_else(|| panic!("no item {name} in {items:#?}"));
    it["signature"].as_str().unwrap().to_string()
}

#[test]
fn cpp_multi_declarator_values_get_own_signatures() {
    let idx = compile_with_items(&[("v.cpp", "int a, *b;\n")]);
    let items = items_of(&idx, "v.cpp");
    assert_eq!(sig_of(&items, None, "a"), "int a");
    assert_eq!(sig_of(&items, None, "b"), "int *b");
}

#[test]
fn cpp_multi_declarator_methods_get_own_signatures() {
    let idx = compile_with_items(&[("b.h", "class B { int f(), *g(); };\n")]);
    let items = items_of(&idx, "b.h");
    assert_eq!(sig_of(&items, Some("B"), "f"), "int f()");
    assert_eq!(sig_of(&items, Some("B"), "g"), "int *g()");
}
