//! Tests for `flow::cpp`.
use crate::flow::testutil::{file, flow_lines, not_drawn};
use crate::model::{Bodiless, Step};

/// The node and edge lines of the chart of `name` in the C++ file `src`.
fn drawn(src: &str, name: &str) -> Vec<String> {
    let f = file("a.cpp", src);
    let i = f.code.items.iter().position(|it| it.name == name).unwrap();
    crate::chart::chart(name, "a.cpp", &f.flows[&i], &|_| String::new())
        .lines()
        .filter(|l| l.starts_with("    n"))
        .map(|l| l.trim().to_string())
        .collect()
}

#[test]
fn an_if_with_an_initializer_and_an_else_if_chain_is_one_branch() {
    let src = "int pick(int v) {\n    // Rule-7. Wide ones first.\n    if (int w = width(v); w > 8) {\n        wide(w);\n    } else if (v < 0) {\n        return negative();\n    } else {\n        narrow();\n    }\n    return 1;\n}\n";
    assert_eq!(
        flow_lines("a.cpp", src, "pick"),
        [
            "branch @3",
            "  pre call width(v) @3",
            "  arm int w = width(v); w > 8 @3 # Rule-7. Wide ones first.",
            "    call wide(w) @4",
            "  arm v < 0 @5",
            "    call negative() @6",
            "    exit return negative(); @6",
            "  arm else @7",
            "    call narrow() @8",
            "exit return 1; @10",
        ]
    );
}

#[test]
fn a_switch_has_an_arm_per_run_of_stacked_labels_each_falling_into_the_next() {
    let src = "int route(int k) {\n    switch (k) {\n    case 4:\n    case 5:\n        left();\n        break;\n    case 6:\n        right();\n        [[fallthrough]];\n    case 7:\n        mid();\n        break;\n    default:\n        return 0;\n    }\n    return 2;\n}\n";
    assert_eq!(
        flow_lines("a.cpp", src, "route"),
        [
            "branch switch (k) @2 breakable",
            "  arm case 4: case 5: @3 falls",
            "    call left() @5",
            "    exit break; @6",
            "  arm case 6: @7 falls",
            "    call right() @8",
            "  arm case 7: @10 falls",
            "    call mid() @11",
            "    exit break; @12",
            "  arm default: @13",
            "    exit return 0; @14",
            "exit return 2; @16",
        ]
    );
}

#[test]
fn a_break_in_a_switch_in_a_loop_leaves_the_switch_and_a_continue_goes_to_the_loop() {
    let src = "void drain(Queue& q) {\n    while (q.ready()) {\n        switch (q.tag()) {\n        case 1: take(); break;\n        default: continue;\n        }\n        settle();\n    }\n}\n";
    assert_eq!(
        drawn(src, "drain"),
        [
            "n0([\"drain\"])",
            "n1>\"q.ready()\"]:::hidden",
            "n0 --> n1",
            "n2{\"while (q.ready())\"}",
            "n1 --> n2",
            "n3>\"q.tag()\"]:::hidden",
            "n2 -->|loop| n3",
            "n4[\"switch (q.tag())\"]",
            "n3 --> n4",
            "n5{\"case 1:\"}",
            "n4 --> n5",
            "n6>\"take()\"]:::hidden",
            "n5 -->|yes| n6",
            "n7[\"break;\"]",
            "n6 --> n7",
            "n8{\"default:\"}",
            "n5 -->|no| n8",
            "n9[\"continue;\"]",
            "n8 -->|yes| n9",
            "n9 --> n1",
            "n10>\"settle()\"]:::hidden",
            "n7 --> n10",
            "n10 --> n1",
            "n11([\"end\"])",
            "n2 -->|done| n11",
        ]
    );
}

#[test]
fn a_continue_in_a_switch_without_default_in_a_for_reaches_the_update() {
    let src = "void scan(int i) {\n    for (; ; i = bump(i)) {\n        switch (i) {\n        case 0: continue;\n        case 1: skip(); break;\n        }\n        look(i);\n    }\n}\n";
    assert_eq!(
        drawn(src, "scan"),
        [
            "n0([\"scan\"])",
            "n1[\"for (; ; i = bump(i))\"]",
            "n0 --> n1",
            "n2[\"switch (i)\"]",
            "n1 -->|loop| n2",
            "n3{\"case 0:\"}",
            "n2 --> n3",
            "n4[\"continue;\"]",
            "n3 -->|yes| n4",
            "n5{\"case 1:\"}",
            "n3 -->|no| n5",
            "n6>\"skip()\"]:::hidden",
            "n5 -->|yes| n6",
            "n7[\"break;\"]",
            "n6 --> n7",
            "n8>\"look(i)\"]:::hidden",
            "n5 -->|no| n8",
            "n7 --> n8",
            "n9>\"bump(i)\"]:::hidden",
            "n8 --> n9",
            "n4 --> n9",
            "n9 --> n1",
        ]
    );
}

#[test]
fn loops_walk_the_initializer_first_the_update_as_post_and_a_do_test_after() {
    let src = "void sweep(Grid& g) {\n    for (int r = first(); r < rows(); r = step(r)) {\n        if (blank(r)) continue;\n        paint(r);\n    }\n    for (auto& c : cells(g)) { mark(c); }\n    while (1) { if (full()) break; }\n    do { tick(); } while (busy());\n}\n";
    assert_eq!(
        flow_lines("a.cpp", src, "sweep"),
        [
            "call first() @2",
            "loop for (int r = first(); r < rows(); r = step(r)) @2",
            "  pre call rows() @2",
            "  branch @3",
            "    pre call blank(r) @3",
            "    arm blank(r) @3",
            "      exit continue; @3",
            "  call paint(r) @4",
            "  post call step(r) @2",
            "call cells(g) @6",
            "loop for (auto& c : cells(g)) @6",
            "  call mark(c) @6",
            "loop while (1) @7",
            "  branch @7",
            "    pre call full() @7",
            "    arm full() @7",
            "      exit break; @7",
            "loop while (busy()) @8 after",
            "  pre call busy() @8",
            "  call tick() @8",
        ]
    );
}

#[test]
fn a_try_has_an_arm_per_catch_and_a_throw_is_an_exit() {
    let src = "int load_all() {\n    try {\n        return fetch();\n    } catch (const Oops& o) {\n        report(o.why());\n        throw;\n    } catch (...) {\n        return -2;\n    }\n}\n";
    assert_eq!(
        flow_lines("a.cpp", src, "load_all"),
        [
            "branch @2",
            "  arm try @2",
            "    call fetch() @3",
            "    exit return fetch(); @3",
            "  arm catch (const Oops& o) @4",
            "    call o.why() @5",
            "    call report(o.why()) @5",
            "    exit throw; @6",
            "  arm catch (...) @7",
            "    exit return -2; @8",
        ]
    );
    assert_eq!(
        not_drawn("a.cpp", src, "load_all"),
        [("try".to_string(), 1)]
    );
}

const TUNE: &str = "void tune(int n) {\n#ifdef TURBO\n    turbo(n);\n#elif defined(ECO)\n    eco(n);\n#else\n    normal(n);\n#endif\n#ifndef QUIET\n    say(n);\n#endif\n    settle();\n}\n";
const TUNE_LINES: [&str; 11] = [
    "branch @2",
    "  arm #ifdef TURBO @2",
    "    call turbo(n) @3",
    "  arm #elif defined(ECO) @4",
    "    call eco(n) @5",
    "  arm else @6",
    "    call normal(n) @7",
    "branch @9",
    "  arm #ifndef QUIET @9",
    "    call say(n) @10",
    "call settle() @12",
];

#[test]
fn a_directive_group_in_a_body_is_a_branch_exhaustive_only_with_an_else() {
    assert_eq!(flow_lines("a.cpp", TUNE, "tune"), TUNE_LINES);
    assert_eq!(not_drawn("a.cpp", TUNE, "tune"), []);
    let f = file("a.cpp", TUNE);
    let exhaustive: Vec<bool> = f.flows[&0]
        .steps
        .iter()
        .filter_map(|s| {
            if let Step::Branch { exhaustive, .. } = s {
                Some(*exhaustive)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(exhaustive, [true, false]);
}

/// `gate`'s braces split across `#ifdef`/`#else`, so the file has a
/// first-branch tree; `tune` after it is whole in the main tree.
const SPLIT: &str = "void gate(int a) {\n#ifdef TURBO\n    if (a) {\n#else\n    if (a && armed()) {\n#endif\n        open();\n    }\n}\n\n";

#[test]
fn a_function_whole_in_the_main_tree_of_a_brace_split_file_is_charted_from_it() {
    let src = format!("{SPLIT}{TUNE}");
    let shifted: Vec<String> = TUNE_LINES
        .iter()
        .map(|l| {
            let (head, at) = l.rsplit_once('@').unwrap();
            format!("{head}@{}", at.parse::<usize>().unwrap() + 10)
        })
        .collect();
    assert_eq!(flow_lines("a.cpp", &src, "tune"), shifted);
    assert_eq!(not_drawn("a.cpp", &src, "tune"), []);
}

#[test]
fn a_brace_split_function_is_charted_from_the_first_branch_with_its_groups_counted() {
    let src = format!("{SPLIT}{TUNE}");
    assert_eq!(
        flow_lines("a.cpp", &src, "gate"),
        ["branch @3", "  arm a @3", "    call open() @7"]
    );
    assert_eq!(not_drawn("a.cpp", &src, "gate"), [("#if".to_string(), 1)]);
}

#[test]
fn a_function_both_trees_list_gets_one_flow() {
    let src = "#ifdef WIDE\nlong span(long a) {\n#else\nint span(int a) {\n#endif\n    return grow(a);\n}\n";
    let f = file("a.cpp", src);
    assert_eq!(f.code.items.len(), 2);
    assert_eq!(f.flows.keys().collect::<Vec<_>>(), [&0]);
    assert!(f.declarations.is_empty());
    assert_eq!(
        flow_lines("a.cpp", src, "span"),
        ["call grow(a) @6", "exit return grow(a); @6"]
    );
    assert_eq!(not_drawn("a.cpp", src, "span"), [("#if".to_string(), 1)]);
}

#[test]
fn a_case_under_a_directive_is_an_arm_and_its_group_is_counted_once() {
    let src = "void dispatch(int op) {\n    switch (op) {\n    case 1:\n        one();\n#ifdef TRACE\n        trace();\n#endif\n        break;\n#if EXTRA\n    case 2:\n        two();\n#else\n    case 3:\n        three();\n#endif\n        both();\n        break;\n    }\n}\n";
    assert_eq!(
        flow_lines("a.cpp", src, "dispatch"),
        [
            "branch switch (op) @2 breakable",
            "  arm case 1: @3 falls",
            "    call one() @4",
            "    branch @5",
            "      arm #ifdef TRACE @5",
            "        call trace() @6",
            "    exit break; @8",
            "  arm case 2: @10 falls",
            "    call two() @11",
            "  arm case 3: @13",
            "    call three() @14",
            "    call both() @16",
            "    exit break; @17",
        ]
    );
    assert_eq!(
        not_drawn("a.cpp", src, "dispatch"),
        [("#if".to_string(), 1)]
    );
}

#[test]
fn lambdas_are_nested_flows_and_a_goto_is_a_counted_exit() {
    let src = "void wire(std::function<void(int)> sink, Pump* p) {\n    auto relay = [&](int u) { if (u) sink(u); };\n    relay(3);\n    spawn([this](int w) { feed(w); });\n    p->prime(1);\n    if (jammed()) goto bail;\n    flow();\nbail:\n    drain();\n}\n";
    assert_eq!(
        flow_lines("a.cpp", src, "wire"),
        [
            "nested relay @2",
            "  branch @2",
            "    arm u @2",
            "      call sink(u) @2",
            "call relay(3) @3",
            "nested closure L4 @4",
            "  call feed(w) @4",
            "call spawn([this](int w) { feed(w); }) @4",
            "call p->prime(1) @5",
            "branch @6",
            "  pre call jammed() @6",
            "  arm jammed() @6",
            "    exit goto bail; @6",
            "call flow() @7",
            "call drain() @9",
        ]
    );
    assert_eq!(not_drawn("a.cpp", src, "wire"), [("goto".to_string(), 1)]);
}

#[test]
fn a_ternary_and_a_short_circuit_are_counted() {
    let src = "int sign(int v) { return v != 0 && known(v) ? up(v) : down(v); }\n";
    assert_eq!(
        flow_lines("a.cpp", src, "sign"),
        [
            "call known(v) @1",
            "call up(v) @1",
            "call down(v) @1",
            "exit return v != 0 && known(v) ? up(v) : down(v); @1",
        ]
    );
    assert_eq!(
        not_drawn("a.cpp", src, "sign"),
        [("&&/||".to_string(), 1), ("ternary".to_string(), 1)]
    );
}

#[test]
fn calls_with_no_site_are_counted_casts_dropped_and_ending_calls_are_exits() {
    let src = "void misc(Fn* fp, int x) {\n    Box* b = new Box(make());\n    (*fp)(x);\n    [&] { poke(); }();\n    Box local(x);\n    static_cast<long>(x);\n    if (x < 0) std::abort();\n    exit(2);\n}\n";
    assert_eq!(
        flow_lines("a.cpp", src, "misc"),
        [
            "call make() @2",
            "nested closure L4 @4",
            "  call poke() @4",
            "branch @7",
            "  arm x < 0 @7",
            "    call std::abort() @7",
            "    exit std::abort() @7",
            "call exit(2) @8",
            "exit exit(2) @8",
        ]
    );
    assert_eq!(
        not_drawn("a.cpp", src, "misc"),
        [("call not traced".to_string(), 3)]
    );
}

#[test]
fn a_body_less_declaration_is_no_flow() {
    let src = "class Kiln {\npublic:\n    Kiln();\n    virtual void fire(int t) = 0;\n    int heat() const { return 9; }\n};\nvoid vent(int);\n";
    let f = file("a.h", src);
    let names = |keys: Vec<usize>| -> Vec<&str> {
        keys.iter()
            .map(|&i| f.code.items[i].name.as_str())
            .collect()
    };
    assert_eq!(names(f.flows.keys().copied().collect()), ["heat"]);
    assert_eq!(
        names(f.declarations.keys().copied().collect()),
        ["Kiln", "fire", "vent"]
    );
    assert!(f.declarations.values().all(|b| *b == Bodiless::Declared));
}

#[test]
fn a_defaulted_or_deleted_function_is_no_flow() {
    let src = "class Pot {\npublic:\n    Pot() = default;\n    Pot(const Pot&) = delete;\n    int size() const { return 1; }\n};\n";
    let f = file("a.h", src);
    let named = |i: &usize| f.code.items[*i].signature.clone();
    assert_eq!(
        f.flows.keys().map(named).collect::<Vec<_>>(),
        ["int size() const"]
    );
    let bodiless: Vec<_> = f.declarations.iter().map(|(i, b)| (named(i), *b)).collect();
    assert_eq!(
        bodiless,
        [
            ("Pot() = default".to_string(), Bodiless::Defaulted),
            ("Pot(const Pot&) = delete".to_string(), Bodiless::Deleted),
        ]
    );
}

#[test]
fn a_case_inside_a_statement_in_a_switch_is_counted_and_its_code_drawn_in_place() {
    let src = "void pump(int n) {\n    switch (n % 2) {\n    case 0:\n        do {\n            lift();\n    case 1:\n            drop();\n        } while (--n > 0);\n    }\n}\n";
    assert_eq!(
        flow_lines("a.cpp", src, "pump"),
        [
            "branch switch (n % 2) @2 breakable",
            "  arm case 0: @3",
            "    loop while (--n > 0) @4 after",
            "      call lift() @5",
            "      call drop() @7",
        ]
    );
    assert_eq!(
        not_drawn("a.cpp", src, "pump"),
        [("nested case".to_string(), 1)]
    );
}

#[test]
fn code_before_the_first_case_is_counted_not_drawn() {
    let src = "void pick(int x) {\n    switch (x) {\n        prep();\n        int k = 2;\n    case 2:\n        two(k);\n    }\n}\n";
    assert_eq!(
        flow_lines("a.cpp", src, "pick"),
        [
            "branch switch (x) @2 breakable",
            "  arm case 2: @5",
            "    call two(k) @6",
        ]
    );
    assert_eq!(
        not_drawn("a.cpp", src, "pick"),
        [("code before the first case".to_string(), 2)]
    );
}
