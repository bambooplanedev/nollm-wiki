//! Tests for `code`.
use super::testutil::{assert_has, assert_lacks};
use super::*;
use crate::model::CallVia;
use std::collections::HashSet;
use tree_sitter::Language;

#[test]
fn ext_of_lowercases_and_source_rank_totally_orders_headers() {
    assert_eq!(ext_of("src/Book.H"), "h");
    assert_eq!(ext_of("src/MAIN.CPP"), "cpp");
    assert_eq!(ext_of("Makefile"), "");
    assert_eq!(ext_of(".gitignore"), "");
    assert_eq!(ext_of("a/b.c/d"), "");

    assert_eq!(source_rank("src/orderbook.h"), 0);
    assert_eq!(source_rank("src/orderbook.hpp"), 1);
    assert_eq!(source_rank("src/orderbook.cpp"), 2);
    assert_eq!(source_rank("src/lib.rs"), 2);
    // Uppercase must rank as its lowercase form, or `.CPP` would outrank `.h`.
    assert_eq!(source_rank("src/Orderbook.H"), 0);
}

#[test]
fn defined_keeps_top_level_names_only() {
    // Module declaration, struct field, trait-impl method, trait with a
    // bound, typed const, free fn. Only the four top-level definitions
    // survive: `a` (Module), `f` (field = Member) and `m` (method =
    // Member) are excluded by `ItemKind`, never by text inspection.
    let src = "pub mod a;\npub struct S { pub f: u8 }\nimpl T for S {\n    fn m(&self) {}\n}\npub trait Tr: Send {}\npub const C: u32 = 1;\npub fn g() {}\n";
    let e = crate::extract("s.rs", src);
    assert_eq!(
        e.defined,
        vec!["C", "S", "Tr", "g"],
        "defined: {:?}",
        e.defined
    );
}

#[test]
fn defined_is_a_sorted_set_and_inline_module_items_are_unqualified() {
    let src = "pub mod inner {\n    pub fn in_mod() {}\n}\npub mod other {\n    pub fn in_mod() {}\n}\npub fn in_mod() {}\n";
    let e = crate::extract("m.rs", src);
    assert_eq!(e.defined, vec!["in_mod"], "defined: {:?}", e.defined);
}

#[test]
fn defined_ignores_trait_impl_headers_and_qualified_methods() {
    let src = "pub struct TextExtractor;\nimpl Extractor for TextExtractor {\n    fn extensions(&self) -> &[&str] { &[] }\n    fn extract(&self, p: &str) -> u8 { 0 }\n}\n";
    let e = crate::extract("text.rs", src);
    assert_eq!(e.defined, vec!["TextExtractor"], "defined: {:?}", e.defined);
}

#[test]
fn methods_keep_inherent_and_trait_declaration_functions_only() {
    // Inherent impl: kept. Trait impl: dropped (owner `<S as T>`). Trait
    // declaration: kept (`function_signature_item`, the trait page
    // defines the API). A field and an associated const are `Member`
    // but not functions. `defined` is unchanged by the new field.
    // `m` collides between the trait impl and the trait declaration, so
    // its presence alone wouldn't pin the trait-impl exclusion; a second
    // trait impl with a uniquely named method (`only_in_impl`) does that
    // instead, via its absence from `e.methods`.
    let src = "pub struct S { pub f: u8 }\nimpl S {\n    pub fn new() -> S { S { f: 0 } }\n    pub fn get(&self) -> u8 { self.f }\n}\nimpl T for S {\n    fn m(&self) {}\n}\nimpl U for S {\n    fn only_in_impl(&self) {}\n}\npub trait T {\n    const K: u32;\n    fn m(&self);\n}\n";
    let e = crate::extract("s.rs", src);
    assert_eq!(
        e.methods,
        vec!["get", "m", "new"],
        "methods: {:?}",
        e.methods
    );
    assert!(
        !e.methods.contains(&"only_in_impl".to_string()),
        "trait-impl methods must be excluded: {:?}",
        e.methods
    );
    assert_eq!(e.defined, vec!["S", "T"], "defined: {:?}", e.defined);
}

#[test]
fn methods_capture_python_class_methods_and_drop_private_ones() {
    // `should_keep` already applies `keep_python_public` to members, so
    // `_private` and `__repr__` never reach `collected`; pinned here so
    // a change there shows up as a `methods` change.
    let py = "class Registry:\n    def register(self, x):\n        pass\n    def _private(self):\n        pass\n    def __repr__(self):\n        return \"\"\n\ndef free():\n    pass\n";
    let e = crate::extract("t.py", py);
    assert_eq!(e.methods, vec!["register"], "methods: {:?}", e.methods);
    assert_eq!(
        e.defined,
        vec!["Registry", "free"],
        "defined: {:?}",
        e.defined
    );
}

#[test]
fn summary_fallback_follows_signature_order_not_grouped_display_order() {
    // `walk.rs`'s real shape: a `pub struct` whose name sorts ahead of a
    // `pub fn`'s under the *grouping* key — uppercase `S` (0x53) sorts
    // before lowercase `w` (0x77), so `SourceFile`'s group leads
    // `## Exports`. If the summary fallback read that same grouped
    // order, the struct would steal the module's summary from the
    // function it's actually about. The fallback must instead compare
    // signatures directly:
    // `pub fn walk() -> u8` sorts before `pub struct SourceFile`
    // lexicographically (`f` < `s`), so it must win the summary even
    // though it displays second in `## Exports`.
    let src = "pub struct SourceFile;\npub fn walk() -> u8 { 0 }\n";
    let e = crate::extract("walk.rs", src);
    assert_eq!(
        e.symbols,
        vec![
            "pub struct SourceFile".to_string(),
            "pub fn walk() -> u8".to_string(),
        ],
        "grouped display order: {:?}",
        e.symbols
    );
    assert_eq!(
        e.summary.as_deref(),
        Some("pub fn walk() -> u8"),
        "the summary must follow signature order, not grouped display order: {:?}",
        e.symbols
    );
}

#[test]
fn multi_line_parameter_lists_are_tidied() {
    let src =
        "pub fn generate(\n    out: &Path,\n    n: usize,\n) -> Result<(), Error> { todo!() }\n";
    let e = crate::extract("t.rs", src);
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "pub fn generate(out: &Path, n: usize) -> Result<(), Error>"),
        "symbols: {:?}",
        e.symbols
    );
}

#[test]
fn one_element_tuple_survives_tidying() {
    let src = "pub type Single = (u8,);\n";
    let e = crate::extract("t.rs", src);
    // a one-element tuple is not a parenthesized value
    assert_has(&e.symbols, "pub type Single = (u8,)");
}

#[test]
fn non_rust_bodies_are_untouched() {
    let src = "def test_visible():\n    pass\n";
    let e = crate::extract("t.py", src);
    assert_eq!(e.body, src);
}

#[test]
fn each_language_keeps_its_own_symbols_and_summary_fallback() {
    // Python now qualifies members and drops function-local definitions.
    let py = "class Registry:\n    def register(self, x):\n        pass\n\ndef outer():\n    def inner():\n        pass\n    return inner\n";
    let e = crate::extract("t.py", py);
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "def Registry.register(self, x)"),
        "symbols: {:?}",
        e.symbols
    );
    assert_has(&e.symbols, "def outer()");
    // function-local def must be dropped
    assert_lacks(&e.symbols, "inner");
    // Python must use `.`, never Rust's `::`
    assert_lacks(&e.symbols, "::");

    // The summary fallback is shared machinery; pin it per language so a
    // reordering cannot move it silently.
    let e = crate::extract("t.py", py);
    assert_eq!(
        e.summary.as_deref(),
        Some("class Registry"),
        "symbols: {:?}",
        e.symbols
    );

    // Go has no owner resolution and no scope guard: a nested func and a
    // method both stay exactly as they are today.
    let go = "package main\n\nfunc Foo(a int) string {\n\treturn \"\"\n}\n";
    let e = crate::extract("m.go", go);
    assert_has(&e.symbols, "func Foo(a int) string");
    assert_eq!(e.summary.as_deref(), Some("func Foo(a int) string"));

    let js = "export function foo(a, b) {\n  return a + b;\n}\n";
    let e = crate::extract("m.js", js);
    assert_eq!(e.symbols, vec!["export function foo(a, b)".to_string()]);
    assert_eq!(e.summary.as_deref(), Some("export function foo(a, b)"));

    let ts = "export class Widget {\n  x = 1;\n}\n";
    let e = crate::extract("m.ts", ts);
    assert_eq!(e.symbols, vec!["export class Widget".to_string()]);
    assert_eq!(e.summary.as_deref(), Some("export class Widget"));
}

#[test]
fn module_stem_is_the_name_importers_use() {
    assert_eq!(module_stem("text.rs"), "text");
    assert_eq!(module_stem("src/formats/extract_rust.rs"), "extract_rust");
    assert_eq!(module_stem("foo.test.rs"), "foo");
    assert_eq!(module_stem("tests/common/mod.rs"), "common");
    assert_eq!(module_stem("pkg/__init__.py"), "pkg");
    // A directory module at the corpus root has no directory to borrow.
    assert_eq!(module_stem("mod.rs"), "mod");
    // The name every page derives from it is the title-cased stem.
    assert_eq!(derive_code_name("tests/common/mod.rs"), "Common");
    assert_eq!(derive_code_name("src/extract_rust.rs"), "Extract Rust");
}

#[test]
fn index_is_a_directory_module_the_way_mod_and_dunder_init_are() {
    // `import './backends/index.js'` names the module `backends`, exactly
    // as `mod common;` names `tests/common/mod.rs`.
    assert_eq!(module_stem("src/backends/index.ts"), "backends");
    assert_eq!(module_stem("web/src/api/index.js"), "api");
    assert_eq!(derive_code_name("src/backends/index.ts"), "Backends");
    // A directory module at the corpus root has no directory to borrow.
    assert_eq!(module_stem("index.ts"), "index");
}

#[test]
fn wrapped_parameter_lists_are_tidied_in_every_language() {
    // `tidy_punctuation` is not gated by `LangSpec` the way owner/vis
    // machinery is — it runs inside `build_signature` for all five
    // languages. Pin that as intended behavior, not something left to be
    // discovered. The fixture also guards the boundary: `beta=(1,)` is
    // real punctuation and must still be tidied to `(1,)`, while
    // `gamma=" )"` is inside a string literal and must survive untouched.
    // One fixture, both sides of the rule.
    let py = "def wrapped(\n    alpha,\n    beta=(1,),\n    gamma=\" )\",\n):\n    pass\n";
    let e = crate::extract("t.py", py);
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "def wrapped(alpha, beta=(1,), gamma=\" )\")"),
        "symbols: {:?}",
        e.symbols
    );

    let go = "package main\n\nfunc Wrapped(\n\ta int,\n\tb int,\n) string {\n\treturn \"\"\n}\n";
    let e = crate::extract("m.go", go);
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "func Wrapped(a int, b int) string"),
        "symbols: {:?}",
        e.symbols
    );

    // The name promises "every language" but the fixtures above cover
    // only Python and Go — JS, TS, and Rust close the gap.
    let js = "export function wrapped(\n  a,\n  b,\n) {\n  return a + b;\n}\n";
    let e = crate::extract("t.js", js);
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "export function wrapped(a, b)"),
        "symbols: {:?}",
        e.symbols
    );

    let ts =
        "export function wrapped(\n  a: number,\n  b: number,\n): number {\n  return a + b;\n}\n";
    let e = crate::extract("t.ts", ts);
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "export function wrapped(a: number, b: number): number"),
        "symbols: {:?}",
        e.symbols
    );

    let rust = "pub fn wrapped(\n    a: u8,\n    b: u8,\n) -> u8 {\n    a + b\n}\n";
    let e = crate::extract("t.rs", rust);
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "pub fn wrapped(a: u8, b: u8) -> u8"),
        "symbols: {:?}",
        e.symbols
    );
}

#[test]
fn javascript_string_literal_line_continuations_survive_collapsing() {
    // Python's `\`-newline join (added so `Z: int = \` reduces to
    // `Z: int`) must NOT be applied to JS/TS: the identical sequence is a
    // legal string-literal continuation there, and a default-parameter
    // value sits inside the retained signature span (the cut stops at
    // `body`, after the parameter list). Joining it would silently turn
    // `"x\<newline>y"` into `"x y"`. `join_continuations` gates this
    // per-language; only the whitespace-collapse (newline -> space) that
    // every language already gets should touch this text.
    let js = "export function foo(a = \"x\\\ny\") {\n  return a;\n}\n";
    let e = crate::extract("t.js", js);
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "export function foo(a = \"x\\ y\")"),
        "the backslash must survive: {:?}",
        e.symbols
    );
}

#[test]
fn every_registered_language_query_compiles() {
    // Forcing the statics IS the check: `QUERIES` panics on a malformed
    // query, and this is the same call `compile` makes before writing any
    // output. A language present in `CODE_EXTENSIONS` but missing a
    // `LangSpec` panics here too.
    validate_queries();
    for ext in CODE_EXTENSIONS {
        let spec = lang_for_ext(ext).unwrap();
        assert!(
            QUERIES.contains_key(&spec.language),
            "no compiled query for {ext}"
        );
    }
}

#[test]
fn the_query_registry_covers_exactly_the_claimed_extensions() {
    // A language accepted by `extensions()` but absent from `QUERIES`
    // would extract nothing while the compile still exits 0 — the silent
    // failure a per-file `Query::new` would allow.
    let claimed: BTreeSet<&str> = CODE_EXTENSIONS.iter().copied().collect();
    let compiled: BTreeSet<&str> = CODE_EXTENSIONS
        .iter()
        .copied()
        .filter(|e| lang_for_ext(e).is_some_and(|s| QUERIES.contains_key(&s.language)))
        .collect();
    assert_eq!(claimed, compiled);
}

#[test]
fn there_is_one_valid_query_per_grammar() {
    validate_queries();
    let grammars: HashSet<Language> = CODE_EXTENSIONS
        .iter()
        .map(|e| lang_for_ext(e).unwrap().language)
        .collect();
    assert_eq!(QUERIES.len(), grammars.len());
}

#[test]
fn extensions_sharing_a_grammar_share_a_query() {
    // Pinning: keying `QUERIES` by grammar is safe only while one grammar
    // means one query string. Not `lang_name`: `ts` and `tsx` are both
    // "typescript" on two grammars, and a query only runs on the grammar
    // it was built for.
    let specs: Vec<LangSpec> = CODE_EXTENSIONS
        .iter()
        .map(|e| lang_for_ext(e).unwrap())
        .collect();
    for a in &specs {
        for b in specs.iter().filter(|b| b.language == a.language) {
            assert_eq!(a.query_src, b.query_src, "query of {}", a.lang_name);
        }
    }
}

#[test]
fn a_doc_comment_must_open_on_the_first_line_for_every_non_cpp_language() {
    // `leading_doc`'s restructuring must not turn Rust, Python, JS, TS or
    // Go into a C++-style banner-skipping scan: a `///` doc comment
    // buried past the top of the file belongs to whatever item follows
    // it, not to the module, and must not become the module's summary.
    let rs = "use crate::graph::build;\n/// Not the module's doc.\npub fn f() {}\n";
    assert_eq!(leading_doc(rs, "rs"), None);

    let js = "const x = 1;\n/// Not the module's doc.\nfunction f() {}\n";
    assert_eq!(leading_doc(js, "js"), None);

    let go = "package main\n// Not the module's doc.\nfunc F() {}\n";
    assert_eq!(leading_doc(go, "go"), None);
}

#[test]
fn a_blank_doc_line_joins_the_paragraph_on_either_side_of_it() {
    // A blank `//!` line is Rust's own paragraph break within one doc
    // comment run, not the end of the comment: aborting there (as the
    // unrestructured `leading_doc` did, via its first-line `?`) lost the
    // elaboration that followed it.
    let rs = "//! Summary line.\n//!\n//! Elaboration line.\nfn f() {}\n";
    assert_eq!(
        leading_doc(rs, "rs").as_deref(),
        Some("Summary line.\nElaboration line.")
    );
}

#[test]
fn is_test_path_follows_the_path_rule() {
    use crate::model::is_test_path;
    for p in [
        "tests/a.rs",
        "src/tests/x.cpp",
        "test_fsm.cpp",
        "fsm_test.go",
        "a/fsm_tests.rs",
    ] {
        assert!(is_test_path(p), "{p}");
    }
    for p in ["src/attest.rs", "src/testing.rs", "contest/a.rs"] {
        assert!(!is_test_path(p), "{p}");
    }
}

fn doc_of(src: &str) -> (Option<String>, Option<String>) {
    let i = crate::extract("src/a.rs", src).items.remove(0);
    (i.doc, i.doc_text)
}

#[test]
fn doc_text_keeps_the_whole_doc() {
    let (doc, text) = doc_of(
            "/// Pure decision.\n///\n/// * `creds_present` — owner has a futures key.\npub fn decide() {}\n",
        );
    assert_eq!(doc.as_deref(), Some("Pure decision."));
    assert_eq!(
        text.as_deref(),
        Some("Pure decision. * `creds_present` — owner has a futures key.")
    );
}

#[test]
fn doc_text_is_none_for_a_one_sentence_doc() {
    assert_eq!(doc_of("/// Only this.\nfn f() {}\n").1, None);
}

#[test]
fn doc_text_caps_on_a_char_boundary() {
    let src = format!("/// {}\nfn f() {{}}\n", "é".repeat(500).replace('é', "éé "));
    let text = doc_of(&src).1.unwrap();
    assert!(
        text.chars().count() <= 1000 && text.ends_with('…'),
        "{}",
        text.len()
    );
}

fn test_flags(path: &str, src: &str) -> Vec<(String, bool)> {
    let e = crate::extract(path, src);
    e.items.into_iter().map(|i| (i.name, i.test)).collect()
}

#[test]
fn rust_items_inside_cfg_test_are_tests() {
    let src = "fn live() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n    fn helper() {}\n}\n";
    let flags = test_flags("src/a.rs", src);
    for (name, want) in [("live", false), ("t", true), ("helper", true)] {
        assert!(
            flags.contains(&(name.to_string(), want)),
            "{name}: {flags:?}"
        );
    }
}

#[test]
fn rust_test_attribute_marks_a_test_outside_cfg_test() {
    let flags = test_flags(
        "src/a.rs",
        "#[tokio::test]\nasync fn x() {}\n#[test]\nfn y() {}\nfn z() {}\n",
    );
    assert_eq!(
        flags,
        [("x".into(), true), ("y".into(), true), ("z".into(), false)]
    );
}

#[test]
fn items_of_a_test_file_are_tests_in_every_language() {
    assert_eq!(
        test_flags("tests/a.go", "package a\nfunc F() {}\n"),
        [("F".into(), true)]
    );
    assert_eq!(
        test_flags("src/a.go", "package a\nfunc F() {}\n"),
        [("F".into(), false)]
    );
}

/// The call sites of item `name` in `src`, one per call, written
/// `name`, `q::name`, `self.name`, `x.name` or `_.name`, sorted.
fn sites(path: &str, src: &str, name: &str) -> Vec<String> {
    let e = crate::extract(path, src);
    let item = e.items.iter().find(|i| i.name == name).expect("the item");
    let mut out: Vec<String> = item
        .call_sites
        .iter()
        .map(|c| match &c.via {
            CallVia::Bare => c.name.clone(),
            CallVia::Path(q) => format!("{q}::{}", c.name),
            CallVia::SelfRecv => format!("self.{}", c.name),
            CallVia::Member(x) => format!("{}.{}", x.as_deref().unwrap_or("_"), c.name),
        })
        .collect();
    out.sort();
    out
}

#[test]
fn call_sites_name_their_targets_in_each_grammar() {
    let rs = "struct S;\nimpl S {\n    fn f(&self) {\n        g();\n        a::b::h();\n        Self::k();\n        self.m();\n        x.n();\n        Vec::<u8>::new();\n        v.iter().map(|y| y);\n    }\n}\nfn g() {}\n";
    assert_eq!(
        sites("src/s.rs", rs, "f"),
        ["Self::k", "Vec::new", "_.map", "b::h", "g", "self.m", "v.iter", "x.n"]
    );
    // The narrowest function holds a call: `g`'s own body is empty.
    assert_eq!(sites("src/s.rs", rs, "g"), [] as [String; 0]);
    let cpp = "void S::f() {\n  g();\n  ns::h();\n  this->m();\n  obj.n();\n  p->q();\n  std::max<int>(1, 2);\n}\n";
    assert_eq!(
        sites("src/s.cpp", cpp, "f"),
        ["g", "ns::h", "obj.n", "p.q", "self.m", "std::max"]
    );
    let py = "class C:\n    def f(self):\n        g()\n        self.m()\n        mod.h()\n";
    assert_eq!(sites("src/c.py", py, "f"), ["g", "mod.h", "self.m"]);
    let ts = "export class C {\n  f() {\n    g();\n    this.m();\n    obj.n();\n  }\n}\n";
    assert_eq!(sites("src/c.ts", ts, "f"), ["g", "obj.n", "self.m"]);
    let go = "package a\nfunc F() {\n\tg()\n\tpkg.H()\n}\n";
    assert_eq!(sites("src/a.go", go, "F"), ["g", "pkg.H"]);
}

#[test]
fn calls_inside_rust_macro_arguments_are_call_sites() {
    let rs = "#[cfg(feature(x))]\nfn f() {\n    let a = format!(\"{}\", crate::port(\"X\", 1));\n    tokio::select! {\n        v = rx.recv() => self.on(v),\n        _ = a.b.tick() => g(1, 2,),\n    }\n    assert!(matches!(x, Foo(y)));\n    vec![h()];\n}\nmacro_rules! m { ($a:expr) => { k($a) } }\n";
    assert_eq!(
        sites("src/s.rs", rs, "f"),
        [
            "Foo",
            "_.tick",
            "crate::port",
            "g",
            "h",
            "rx.recv",
            "self.on"
        ]
    );
    let e = crate::extract("src/s.rs", rs);
    let f = e.items.iter().find(|i| i.name == "f").unwrap();
    let args = |n: &str| f.call_sites.iter().find(|c| c.name == n).unwrap().args;
    assert_eq!(
        (args("port"), args("g"), args("tick")),
        (Some(2), Some(2), Some(0))
    );
    let rs = "fn f() {\n    assert_eq!(a::b::g(1), Foo::<T>::k(2), h::<A, B>(3));\n    m! { fn helper(a: u32) {} struct P(u32, u32); }\n    println!(\"{}\", xs.fold(0, |a, b| a + b), q(V::<K, W>::new()));\n}\n";
    assert_eq!(
        sites("src/s.rs", rs, "f"),
        ["Foo::k", "V::new", "b::g", "h", "q", "xs.fold"]
    );
    let e = crate::extract("src/s.rs", rs);
    let f = e.items.iter().find(|i| i.name == "f").unwrap();
    let args = |n: &str| f.call_sites.iter().find(|c| c.name == n).unwrap().args;
    assert_eq!((args("g"), args("fold"), args("q")), (Some(1), None, None));
}

#[test]
fn calls_through_an_import_alias_name_the_original() {
    let ts = "import { stopJob as apiStop, b } from './api/client';\nimport * as auth from '../api/auth.ts';\nfunction f() {\n  apiStop(1);\n  auth.login();\n  b();\n}\n";
    assert_eq!(sites("src/h.ts", ts, "f"), ["auth.login", "b", "stopJob"]);
    let py = "import a.b.mod as m\nfrom p import f as g\ndef h():\n    m.run()\n    g()\n";
    assert_eq!(sites("src/h.py", py, "h"), ["f", "mod.run"]);
    let rs = "use crate::x::{init as manager_init, y as _};\nuse a::b as c;\nfn h() {\n    manager_init();\n    c::k();\n}\n";
    assert_eq!(sites("src/h.rs", rs, "h"), ["b::k", "init"]);
    let go = "package a\nimport (\n\tx \"a/b/c\"\n)\nfunc F() {\n\tx.G()\n}\n";
    assert_eq!(sites("src/a.go", go, "F"), ["c.G"]);
    // A parameter or receiver of the same name shadows the alias.
    let go = "package a\nimport m \"p/models/v2\"\nfunc (m *T) F() {\n\tm.Save()\n}\nfunc G() {\n\tm.Save()\n}\n";
    assert_eq!(sites("src/a.go", go, "F"), ["m.Save"]);
    assert_eq!(sites("src/a.go", go, "G"), ["models.Save"]);
    let ts = "import * as api from './api/index';\nimport { default as d } from './d';\nfunction f() {\n  api.go();\n  d();\n}\n";
    assert_eq!(sites("src/h.ts", ts, "f"), ["api.go", "d"]);
    let rs = "use crate::net::{self as n};\nfn h() {\n    n::connect();\n}\n";
    assert_eq!(sites("src/h.rs", rs, "h"), ["net::connect"]);
}
