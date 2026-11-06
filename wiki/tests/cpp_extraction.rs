use std::fs;

mod common;

fn compile_tree(files: &[(&str, &str)]) -> serde_json::Value {
    compile_with_items(files).0
}

/// The compiled `index.json` and its `items.json` sidecar.
fn compile_with_items(files: &[(&str, &str)]) -> (serde_json::Value, serde_json::Value) {
    let (_dir, _, out) = common::compiled(files);
    let items = fs::read_to_string(out.join("items.json")).unwrap();
    (common::index(&out), serde_json::from_str(&items).unwrap())
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

#[test]
fn a_pointer_or_reference_returning_definition_pairs_with_its_declaration() {
    let idx = compile_with_items(&[
        (
            "include/router.h",
            "class Router {\npublic:\n  /// Picks a venue.\n  const Venue* pick_venue(int id);\n  Venue& front();\n};\n",
        ),
        (
            "src/router.cpp",
            "#include \"router.h\"\nconst Venue* Router::pick_venue(int id) { return nullptr; }\nVenue& Router::front() { return *pick_venue(0); }\n",
        ),
    ]);
    let h = items_of(&idx, "include/router.h");
    let at = |items: &[serde_json::Value], n: &str| items.iter().find(|i| i["name"] == n).cloned();
    assert_eq!(at(&h, "pick_venue").unwrap()["twin"], "src/router.cpp:2-2");
    assert_eq!(at(&h, "front").unwrap()["twin"], "src/router.cpp:3-3");
    let c = items_of(&idx, "src/router.cpp");
    let pick = at(&c, "pick_venue").expect("definition named without its qualifier");
    assert_eq!(pick["owner"], "Router");
    assert_eq!(pick["doc"], "Picks a venue.");
    assert_eq!(pick["twin"], "include/router.h:3-4");
}

/// The items of `path` named `name`, in source order.
fn named(
    idx: &(serde_json::Value, serde_json::Value),
    path: &str,
    name: &str,
) -> Vec<serde_json::Value> {
    let found: Vec<_> = items_of(idx, path)
        .into_iter()
        .filter(|i| i["name"] == name)
        .collect();
    assert!(!found.is_empty(), "no {name} in {path}");
    found
}

#[test]
fn a_definition_in_a_different_stem_file_pairs_when_its_key_is_unique() {
    let idx = compile_with_items(&[
        (
            "src/box.hpp",
            "class Box {\npublic:\n  /// Apart.\n  int apart(int n) const;\n};\n",
        ),
        (
            "src/box_impl.cpp",
            "int Box::apart(int n) const { return n; }\n",
        ),
    ]);
    let h = &named(&idx, "src/box.hpp", "apart")[0];
    assert_eq!(h["twin"], "src/box_impl.cpp:1-1");
    let c = &named(&idx, "src/box_impl.cpp", "apart")[0];
    assert_eq!(c["twin"], "src/box.hpp:3-4");
    assert_eq!(c["doc"], "Apart.");
}

#[test]
fn a_public_header_pairs_with_definitions_under_src() {
    let idx = compile_with_items(&[
        (
            "include/lib/api.h",
            "void api_open(int fd);\nvoid api_reset(int fd);\nclass Book {\npublic:\n  const Level* find(int px) const;\n  Level* find(int px);\n};\n",
        ),
        ("src/internal.h", "void api_reset(int fd);\n"),
        (
            "src/open.cc",
            "void api_open(int fd) {}\nvoid api_reset(int fd) {}\n",
        ),
        (
            "src/book_find.cc",
            "const Level* Book::find(int px) const { return nullptr; }\nLevel* Book::find(int px) { return nullptr; }\n",
        ),
    ]);
    assert_eq!(
        named(&idx, "include/lib/api.h", "api_open")[0]["twin"],
        "src/open.cc:1-1"
    );
    // A key declared in two headers stays unpaired (Review Focus 2).
    for path in ["include/lib/api.h", "src/internal.h", "src/open.cc"] {
        for it in named(&idx, path, "api_reset") {
            assert!(it.get("twin").is_none(), "{path}: {it}");
        }
    }
    // Same-arity overloads stay unpaired across files (Review Focus 1).
    for path in ["include/lib/api.h", "src/book_find.cc"] {
        let finds = named(&idx, path, "find");
        assert_eq!(finds.len(), 2, "{path}");
        for it in finds {
            assert!(it.get("twin").is_none(), "{path}: {it}");
        }
    }
}

#[test]
fn two_definitions_of_one_key_never_pair() {
    let idx = compile_with_items(&[
        (
            "src/thread.h",
            "void spawn(int n);\nvoid join(int n);\nvoid wait(int n);\n",
        ),
        (
            "src/posix/thread_posix.cc",
            "void spawn(int n) {}\nvoid join(int n) {}\n",
        ),
        ("src/win/thread_win.cc", "void spawn(int n) {}\n"),
        (
            "src/wait.cc",
            "#ifdef _WIN32\nvoid wait(int n) {}\n#else\nvoid wait(int n) {}\n#endif\n",
        ),
    ]);
    assert_eq!(
        named(&idx, "src/thread.h", "join")[0]["twin"],
        "src/posix/thread_posix.cc:2-2"
    );
    for (path, name) in [
        ("src/thread.h", "spawn"),
        ("src/thread.h", "wait"),
        ("src/posix/thread_posix.cc", "spawn"),
        ("src/win/thread_win.cc", "spawn"),
        ("src/wait.cc", "wait"),
    ] {
        for it in named(&idx, path, name) {
            assert!(it.get("twin").is_none(), "{path} {name}: {it}");
        }
    }
    assert_eq!(named(&idx, "src/wait.cc", "wait").len(), 2);
}
