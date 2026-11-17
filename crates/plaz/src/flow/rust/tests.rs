//! Tests for `flow::rust`.
use crate::flow::testutil::{file, flow_lines, not_drawn};

#[test]
fn rust_flow_draws_if_chains_matches_loops_and_exits() {
    let src = "fn f(x: u8, cb: impl Fn()) -> u8 {\n    // REQ-417. Small inputs skip the cache.\n    // The cache is cold at start.\n    if x < 3 {\n        cb();\n        return 0;\n    } else if let Some(y) = g(x) {\n        h(y)?;\n    } else {\n        k();\n    }\n    for i in 0..x {\n        if i == 2 { break; }\n    }\n    match x {\n        1 | 2 => m(),\n        n if n > 9 => panic!(\"big\"),\n        _ => {}\n    }\n    x\n}\n";
    assert_eq!(
        flow_lines("src/a.rs", src, "f"),
        [
            "branch @4",
            "  arm x < 3 @4 # REQ-417. Small inputs skip the cache.\\nThe cache is cold at start.",
            "    call cb() @5",
            "    exit return 0 @6",
            "  pre call g(x) @7",
            "  arm let Some(y) = g(x) @7",
            "    call h(y) @8",
            "  arm else @9",
            "    call k() @10",
            "loop for i in 0..x @12",
            "  branch @13",
            "    arm i == 2 @13",
            "      exit break @13",
            "branch match x @15",
            "  arm 1 | 2 @16",
            "    call m() @16",
            "  arm n if n > 9 @17",
            "    exit panic!(\"big\") @17",
            "  arm _ @18",
        ]
    );
    assert_eq!(not_drawn("src/a.rs", src, "f"), [("?".to_string(), 1)]);
}

#[test]
fn rust_flow_draws_let_else_as_a_decision() {
    let src = "fn f(o: Option<u8>) -> u8 {\n    let Some(a) = o else { return 0; };\n    let Some(b) = g(a) else { panic!(\"no\") };\n    if let Some(c) = h(b) { k(c); } else { m(); }\n    while let Some(d) = n() { p(d); }\n    a\n}\n";
    assert_eq!(
        flow_lines("src/a.rs", src, "f"),
        [
            "branch @2",
            "  arm let Some(a) = o @2",
            "  arm else @2",
            "    exit return 0 @2",
            "branch @3",
            "  pre call g(a) @3",
            "  arm let Some(b) = g(a) @3",
            "  arm else @3",
            "    exit panic!(\"no\") @3",
            "branch @4",
            "  pre call h(b) @4",
            "  arm let Some(c) = h(b) @4",
            "    call k(c) @4",
            "  arm else @4",
            "    call m() @4",
            "loop while let Some(d) = n() @5",
            "  pre call n() @5",
            "  call p(d) @5",
        ]
    );
}

#[test]
fn rust_flow_counts_unwrap_or_and_expression_closures_and_nests_block_closures() {
    let src = "fn f(v: Vec<u8>) -> u8 {\n    let a = v.first().copied().unwrap_or(0);\n    let b: Vec<u8> = v.iter().map(|y| y + a).collect();\n    let run = |z: u8| {\n        if z > a { log(z); }\n    };\n    run(a);\n    a\n}\n";
    let lines = flow_lines("src/a.rs", src, "f");
    assert_eq!(
        lines,
        [
            "call v.first() @2",
            "call v.first().copied() @2",
            "call v.first().copied().unwrap_or(0) @2",
            "call v.iter() @3",
            "call v.iter().map(|y| y + a) @3",
            "call v.iter().map(|y| y + a).collect() @3",
            "nested run @4",
            "  branch @5",
            "    arm z > a @5",
            "      call log(z) @5",
            "call run(a) @7",
        ]
    );
    assert_eq!(
        not_drawn("src/a.rs", src, "f"),
        [("closure".to_string(), 1), ("unwrap_or".to_string(), 1)]
    );
}

#[test]
fn tests_and_broken_functions_are_handled_not_guessed() {
    let f = file(
        "src/a.rs",
        "fn f() { if a() { b(); } }\n#[test]\nfn t() { if x() {} }\n",
    );
    assert_eq!(f.flows.len(), 1);
    // a damaged function that tree-sitter still recognises carries the flag
    let g = file("src/b.rs", "fn ok() {}\nfn g() { if a() { b(; } }\n");
    assert!(g.syntax_errors);
    let i = g.code.items.iter().position(|it| it.name == "g").unwrap();
    assert!(g.flows[&i].syntax_errors);
    assert!(g.flows.values().any(|fl| !fl.syntax_errors)); // `ok` is still charted and clean
}

#[test]
fn two_functions_on_one_line_get_their_own_flows() {
    let src = "fn a() { x(); } fn a2() { if c() { y(); } }\n";
    assert_eq!(file("src/a.rs", src).flows.len(), 2);
    assert_eq!(flow_lines("src/a.rs", src, "a"), ["call x() @1"]);
    // `a2` holds only its own `if`: the condition call, then the one arm; no `x()` of `a`.
    assert_eq!(
        flow_lines("src/a.rs", src, "a2"),
        [
            "branch @1",
            "  pre call c() @1",
            "  arm c() @1",
            "    call y() @1"
        ]
    );
}

#[test]
fn each_call_reaches_nested_flows_with_their_scope_chain() {
    let f = file(
        "src/a.rs",
        "fn f(cb: impl Fn()) {\n    let run = |z: u8| {\n        cb();\n    };\n    run(1);\n}\n",
    );
    let mut seen = Vec::new();
    f.flows[&0].each_call(&mut |scopes, c| {
        seen.push((
            c.text.clone(),
            scopes.len(),
            scopes[0].locals.contains("cb"),
        ));
    });
    assert_eq!(
        seen,
        [
            ("cb()".to_string(), 2, true),
            ("run(1)".to_string(), 1, true)
        ]
    );
}

#[test]
fn generic_parameters_and_items_of_the_body_are_rebound_not_locals() {
    let src = "struct W<T>(T);\nimpl<T> W<T> {\n    fn f<'a, U: Into<u8>, const N: usize>(&self, t: T, u: U, s: S) {\n        fn inner() {}\n        struct L;\n    }\n}\n";
    let f = file("src/a.rs", src);
    let fl = f.flows.values().next().unwrap();
    assert_eq!(
        fl.rebound,
        ["L", "N", "T", "U", "inner"].map(String::from).into()
    );
    assert_eq!(fl.locals, ["s", "t", "u"].map(String::from).into());
    assert_eq!(fl.types, [("s".to_string(), "S".to_string())].into());
}

#[test]
fn a_name_bound_with_two_types_has_no_type() {
    let f = file(
            "src/a.rs",
            "fn f() {\n    let s: Store = make();\n    let s: Cache = other();\n    let t: Store = make();\n}\n",
        );
    let fl = &f.flows[&0];
    assert_eq!(fl.types.get("s"), None);
    assert_eq!(fl.types.get("t").map(String::as_str), Some("Store"));
}

#[test]
fn rust_bindings_cover_patterns_closures_and_typed_lets() {
    let src = "fn f(a: &mut Vec<u8>, (b, c): (u8, u8)) {\n    let d: Foo = Foo {};\n    let e = Bar { x: 1 };\n    let g = |y| y;\n    if let Some(h) = a.pop() {}\n    for (i, j) in a.iter().enumerate() {}\n}\n";
    let fl = file("src/a.rs", src).flows[&0].clone();
    assert_eq!(
        fl.locals,
        ["a", "b", "c", "d", "e", "g", "h", "i", "j", "y"]
            .map(String::from)
            .into()
    );
    assert_eq!(
        fl.types,
        [("a", "Vec<u8>"), ("d", "Foo"), ("e", "Bar")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .into()
    );
}

#[test]
fn an_inner_fn_is_drawn_nested_and_shorthand_fields_bind() {
    let src = "fn f(p: P) {\n    fn inner() { q(); }\n    if let P { a, b: c } = p {}\n}\n";
    assert_eq!(
        flow_lines("src/a.rs", src, "f")[..2],
        ["nested inner @2", "  call q() @2"]
    );
    let fl = file("src/a.rs", src).flows[&0].clone();
    assert_eq!(fl.locals, ["a", "c", "p"].map(String::from).into());
}

#[test]
fn an_else_after_a_comment_is_kept_and_a_body_comment_stays_in_its_arm() {
    let src = "fn f() {\n    if a() {\n        x();\n        // done\n    } else /* c */ {\n        k();\n    }\n    if b() {\n    } else // note\n    if c() {\n        m();\n    }\n    match a() {\n        1 => {\n            // in one\n        }\n        _ => k(),\n    }\n}\n";
    assert_eq!(
        flow_lines("src/a.rs", src, "f"),
        [
            "branch @2",
            "  pre call a() @2",
            "  arm a() @2",
            "    call x() @3",
            "  arm else @5 # c",
            "    call k() @6",
            "branch @8",
            "  pre call b() @8",
            "  arm b() @8",
            "  pre call c() @10",
            "  arm c() @10",
            "    call m() @11",
            "call a() @13",
            "branch match a() @13",
            "  arm 1 @14",
            "  arm _ @17",
            "    call k() @17",
        ]
    );
}
