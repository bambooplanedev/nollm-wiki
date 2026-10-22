use std::fs;

fn compile_tree(files: &[(&str, &str)]) -> serde_json::Value {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    for (path, body) in files {
        let p = src.path().join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    wiki::compile(src.path(), out.path(), &wiki::CompileOptions::default()).unwrap();
    serde_json::from_str(&fs::read_to_string(out.path().join("index.json")).unwrap()).unwrap()
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
