//! `.c` is indexed like `.h` (the C++ spec); `.mjs`/`.cjs` like `.js`.
use std::fs;

mod common;

fn names(items: &serde_json::Value, id: &str) -> Vec<String> {
    items[id]
        .as_array()
        .unwrap_or_else(|| panic!("no items for {id}: {items:#}"))
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_string())
        .collect()
}

fn items(out: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(out.join("items.json")).unwrap()).unwrap()
}

#[test]
fn c_files_get_callers_and_include_edges_with_their_headers() {
    let (_d, w) = common::compiled_wiki(&[
        ("src/util.h", "int add(int a, int b);\n"),
        (
            "src/util.c",
            "#include \"util.h\"\nint add(int a, int b) {\n  return a + b;\n}\n",
        ),
        (
            "src/main.c",
            "#include \"util.h\"\nint main(void) {\n  return add(1, 2);\n}\n",
        ),
    ]);
    let out = w.callers("util_c", "add", None, None).unwrap();
    assert!(out.contains("main"), "{out}");
    let (_d, _, out) = common::compiled(&[
        ("src/util.h", "int add(int a, int b);\n"),
        (
            "src/main.c",
            "#include \"util.h\"\nint main(void) {\n  return add(1, 2);\n}\n",
        ),
    ]);
    let idx = common::index(&out);
    let main = idx["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "src/main.c")
        .unwrap();
    assert!(
        main["neighbors_out"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "util"),
        "{main:#}"
    );
}

#[test]
fn mjs_and_cjs_are_indexed_like_js_and_imports_resolve() {
    let (_d, _, out) = common::compiled(&[
        ("src/lib.js", "export function helper() {}\n"),
        (
            "src/app.mjs",
            "import { helper } from './lib.js';\nexport function run() { helper(); }\n",
        ),
        (
            "src/old.cjs",
            "function legacy() {}\nmodule.exports = { legacy };\n",
        ),
    ]);
    let it = items(&out);
    assert!(names(&it, "app").contains(&"run".to_string()), "{it:#}");
    assert!(names(&it, "old").contains(&"legacy".to_string()), "{it:#}");
    let idx = common::index(&out);
    let app = idx["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "src/app.mjs")
        .unwrap();
    assert!(
        app["neighbors_out"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "lib"),
        "{app:#}"
    );
}

/// Known limit: C++ keywords used as C identifiers. Measured: the functions
/// around them survive, and `new`/`class` even surface as symbols themselves.
#[test]
fn c_with_cpp_keyword_identifiers_known_limit() {
    let (_d, _, out) = common::compiled(&[(
        "k.c",
        "int before(void) { return 0; }\nint new = 1;\nstruct class { int x; };\nint takes_this(int this) { return this; }\nint after(void) { return 1; }\n",
    )]);
    assert_eq!(
        names(&items(&out), "k"),
        ["before", "new", "class", "takes_this", "after"]
    );
}
