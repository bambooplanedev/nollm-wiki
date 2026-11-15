//! Tests for `flow::python`.
use crate::flow::testutil::{file, flow_lines, not_drawn};

#[test]
fn python_flow_draws_elif_match_and_nested_defs() {
    let src = "def f(x, ask):\n    # Tag-12. Retries only on 5xx.\n    if x > 500:\n        ask(x)\n    elif x == 404:\n        raise NotFound()\n    else:\n        pass\n    def inner(y):\n        if y:\n            return log(y)\n    match x:\n        case 1 | 2:\n            g()\n        case _:\n            pass\n    return inner(x) if x else None\n";
    assert_eq!(
        flow_lines("a.py", src, "f"),
        [
            "branch @3",
            "  arm x > 500 @3 # Tag-12. Retries only on 5xx.",
            "    call ask(x) @4",
            "  arm x == 404 @5",
            "    call NotFound() @6",
            "    exit raise NotFound() @6",
            "  arm else @7",
            "nested inner @9",
            "  branch @10",
            "    arm y @10",
            "      call log(y) @11",
            "      exit return log(y) @11",
            "branch match x @12",
            "  arm 1 | 2 @13",
            "    call g() @14",
            "  arm _ @15",
            "call inner(x) @17",
            "exit return inner(x) if x else None @17",
        ]
    );
    assert_eq!(not_drawn("a.py", src, "f"), [("ternary".to_string(), 1)]);
}

#[test]
fn python_flow_draws_loops_and_try_in_order_and_counts_what_it_does_not_draw() {
    let src = "def g(xs, ok):\n    while more(xs):\n        if ok:\n            break\n        step()\n    for x in make(xs):\n        if x:\n            continue\n    else:\n        done()\n    try:\n        return load(xs)\n    except ValueError as e:\n        warn(e)\n    finally:\n        close()\n    return ok and check(xs) or [y for y in xs] or (lambda: z())\n";
    assert_eq!(
        flow_lines("a.py", src, "g"),
        [
            "loop while more(xs) @2",
            "  pre call more(xs) @2",
            "  branch @3",
            "    arm ok @3",
            "      exit break @4",
            "  call step() @5",
            "call make(xs) @6",
            "loop for x in make(xs) @6",
            "  branch @7",
            "    arm x @7",
            "      exit continue @8",
            "  else",
            "    call done() @10",
            "branch @11",
            "  arm try @11",
            "    call load(xs) @12",
            "    exit return load(xs) @12",
            "  arm except ValueError as e @13",
            "    call warn(e) @14",
            "finally @15",
            "  call close() @16",
            "call check(xs) @17",
            "call z() @17",
            "exit return ok and check(xs) or [y for y in xs] or (lambda: z()) @17",
        ]
    );
    assert_eq!(
        not_drawn("a.py", src, "g"),
        [
            ("&&/||".to_string(), 3),
            ("closure".to_string(), 2),
            ("try".to_string(), 1)
        ]
    );
}

#[test]
fn a_loop_else_belongs_to_its_loop_and_a_finally_follows_its_try() {
    let src = "def k(xs):\n    for x in xs:\n        if x:\n            break\n    else:\n        none()\n    try:\n        return a()\n    finally:\n        b()\n";
    assert_eq!(
        flow_lines("a.py", src, "k"),
        [
            "loop for x in xs @2",
            "  branch @3",
            "    arm x @3",
            "      exit break @4",
            "  else",
            "    call none() @6",
            "call a() @8",
            "exit return a() @8",
            "finally @9",
            "  call b() @10",
        ]
    );
    assert_eq!(not_drawn("a.py", src, "k"), [("try".to_string(), 1)]);
}

#[test]
fn a_try_else_runs_after_the_body_not_after_a_handler() {
    let src = "def h():\n    try:\n        a()\n    except E:\n        b()\n    else:\n        c()\n    finally:\n        d()\n";
    assert_eq!(
        flow_lines("a.py", src, "h"),
        [
            "branch @2",
            "  arm try @2",
            "    call a() @3",
            "  arm except E @4",
            "    call b() @5",
            "  after try",
            "    call c() @7",
            "finally @8",
            "  call d() @9",
        ]
    );
}

#[test]
fn python_bindings_cover_every_binding_form_and_rebind_body_defs_and_imports() {
    let src = "def f(a: Store, b: mod.Cache, *args, c=1, d: int = 2, **kw):\n    import json, os.path as p\n    from .m import h as g2\n    e = Store()\n    k: Cache = load()\n    q, (r, *s) = pair()\n    t += 1\n    obj.attr = 1\n    items[0] = 2\n    for i, j in a.items():\n        pass\n    with open(a) as fh:\n        pass\n    try:\n        pass\n    except E as err:\n        pass\n    if (w := a.get()):\n        pass\n    [v for v in a]\n    global gl\n    def inner(z):\n        zz = 1\n    class L:\n        cl = 1\n    l = L()\n    match a:\n        case [m1, m2]:\n            pass\n";
    let fl = &file("a.py", src).flows[&0];
    assert_eq!(
        fl.locals,
        [
            "a", "args", "b", "c", "d", "e", "err", "fh", "gl", "i", "j", "k", "kw", "l", "m1",
            "m2", "q", "r", "s", "t", "v", "w"
        ]
        .map(String::from)
        .into()
    );
    assert_eq!(
        fl.types,
        [
            ("a", "Store"),
            ("b", "mod::Cache"),
            ("d", "int"),
            ("e", "Store"),
            ("k", "Cache")
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .into()
    );
    assert_eq!(
        fl.rebound,
        ["L", "g2", "inner", "json", "p"].map(String::from).into()
    );
}
