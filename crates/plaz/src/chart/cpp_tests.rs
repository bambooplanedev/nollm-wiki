//! Tests for `flow::cpp` that draw charts.
use crate::flow::testutil::{file, not_drawn};

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
            "n5{\"case 1#58;\"}",
            "n4 --> n5",
            "n6>\"take()\"]:::hidden",
            "n5 -->|yes| n6",
            "n7[\"break;\"]",
            "n6 --> n7",
            "n8{\"default#58;\"}",
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
            "n3{\"case 0#58;\"}",
            "n2 --> n3",
            "n4[\"continue;\"]",
            "n3 -->|yes| n4",
            "n5{\"case 1#58;\"}",
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

/// `gate`'s braces split across `#ifdef`/`#else`, so the file has a
/// first-branch tree. A copy of the const in `wiki-code`'s `links/flow/cpp/tests.rs`.
const SPLIT: &str = "void gate(int a) {\n#ifdef TURBO\n    if (a) {\n#else\n    if (a && armed()) {\n#endif\n        open();\n    }\n}\n\n";

#[test]
fn a_first_branch_group_with_other_branches_stays_counted_and_drawn_in_line() {
    let src = format!("void mix(float * MY_RESTRICT s) {{\n#if defined(USE_A)\n    {{ fast_mix(s); }}\n#elif defined(USE_B)\n    wide_mix(s);\n#else\n    plain_mix(s);\n#endif\n    finish(s);\n}}\n\n{SPLIT}");
    assert_eq!(
        drawn(&src, "mix"),
        [
            "n0([\"mix\"])",
            "n1>\"fast_mix(s)\"]:::hidden",
            "n0 --> n1",
            "n2>\"finish(s)\"]:::hidden",
            "n1 --> n2",
            "n3([\"end\"])",
            "n2 --> n3",
        ]
    );
    assert_eq!(not_drawn("a.cpp", &src, "mix"), [("#if".to_string(), 1)]);
}

#[test]
fn a_first_branch_that_cannot_fall_through_without_a_final_exit_leaves_by_an_unseen_way() {
    let src = format!("int fold(float * MY_RESTRICT s, int n) {{\n#if defined(USE_A)\n    if (n) {{\n        return 1;\n    }} else {{\n        return 2;\n    }}\n#else\n    lean(s);\n#endif\n    rest(s);\n    return 0;\n}}\n\nint pick(float * MY_RESTRICT s, int n) {{\n#ifdef USE_B\n    switch (n) {{\n    case 1: return 1;\n    default: return 2;\n    }}\n#else\n    lean(s);\n#endif\n    rest(s);\n    return 0;\n}}\n\n{SPLIT}");
    assert_eq!(
        drawn(&src, "fold"),
        [
            "n0([\"fold\"])",
            "n1{\"#35;if defined(USE_A)\"}",
            "n0 --> n1",
            "n2{\"n\"}",
            "n1 -->|yes| n2",
            "n3([\"return 1;\"])",
            "n2 -->|yes| n3",
            "n4([\"return 2;\"])",
            "n2 -->|no| n4",
            "n5>\"rest(s)\"]:::hidden",
            "n1 -->|exit not drawn| n5",
            "n6([\"return 0;\"])",
            "n5 --> n6",
        ]
    );
    let pick = drawn(&src, "pick");
    assert_eq!(
        pick.iter()
            .filter(|l| l.contains("rest(s)") || l.contains("exit not drawn"))
            .collect::<Vec<_>>(),
        ["n7>\"rest(s)\"]:::hidden", "n1 -->|exit not drawn| n7"]
    );
}

#[test]
fn a_first_branch_that_cannot_fall_through_with_nothing_after_its_group_has_no_unseen_way() {
    let src = format!("int last(float * MY_RESTRICT s) {{\n#ifdef USE_A\n    fast(s);\n    return 1;\n#else\n    slow(s);\n    return 2;\n#endif\n}}\n\n{SPLIT}");
    assert_eq!(
        drawn(&src, "last"),
        [
            "n0([\"last\"])",
            "n1>\"fast(s)\"]:::hidden",
            "n0 --> n1",
            "n2([\"return 1;\"])",
            "n1 --> n2",
        ]
    );
}

#[test]
fn a_first_branch_ending_in_return_beside_unseen_branches_leaves_by_an_unseen_way() {
    let src = format!("int pick(float * MY_RESTRICT s) {{\n#if defined(USE_A)\n    {{ fast(s); return 1; }}\n#else\n    {{ slow(s); return 2; }}\n#endif\n    tail(s);\n    return 0;\n}}\n\n{SPLIT}");
    assert_eq!(
        drawn(&src, "pick"),
        [
            "n0([\"pick\"])",
            "n1{\"#35;if defined(USE_A)\"}",
            "n0 --> n1",
            "n2>\"fast(s)\"]:::hidden",
            "n1 -->|yes| n2",
            "n3([\"return 1;\"])",
            "n2 --> n3",
            "n4>\"tail(s)\"]:::hidden",
            "n1 -->|exit not drawn| n4",
            "n5([\"return 0;\"])",
            "n4 --> n5",
        ]
    );
    assert_eq!(not_drawn("a.cpp", &src, "pick"), [("#if".to_string(), 1)]);
}

#[test]
fn a_first_branch_group_in_an_unbraced_case_arm_leaves_by_an_unseen_way() {
    let src = format!("int sel(float * MY_RESTRICT s, int n) {{\n    switch (n) {{\n    case 1:\n#ifdef USE_A\n        if (n) return 1; else return 2;\n#else\n        lean(s);\n#endif\n        mid(s);\n        break;\n    default:\n        rest(s);\n    }}\n    return 0;\n}}\n\n{SPLIT}");
    let lines = drawn(&src, "sel");
    let mid = lines.iter().find(|l| l.contains("mid(s)")).unwrap();
    let id = mid.split(['>', '[']).next().unwrap();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.ends_with(&format!(" {id}")))
            .collect::<Vec<_>>(),
        ["n3 -->|exit not drawn| n7"]
    );
    assert_eq!(not_drawn("a.cpp", &src, "sel"), [("#if".to_string(), 1)]);
}

#[test]
fn a_do_loop_whose_test_never_passes_runs_its_body_once() {
    // `continue` goes to the test, which ends the loop, as `break` does.
    let src = "void once(int k) {\n    do {\n        if (k) continue;\n        if (k > 1) break;\n        work();\n    } while (0);\n    done();\n}\n";
    assert_eq!(
        drawn(src, "once"),
        [
            "n0([\"once\"])",
            "n1{\"k\"}",
            "n0 --> n1",
            "n2[\"continue;\"]",
            "n1 -->|yes| n2",
            "n3{\"k #gt; 1\"}",
            "n1 -->|no| n3",
            "n4[\"break;\"]",
            "n3 -->|yes| n4",
            "n5>\"work()\"]:::hidden",
            "n3 -->|no| n5",
            "n6{\"while (0)\"}",
            "n5 --> n6",
            "n2 --> n6",
            "n7>\"done()\"]:::hidden",
            "n4 --> n7",
            "n6 -->|done| n7",
            "n8([\"end\"])",
            "n7 --> n8",
        ]
    );
    let src = src
        .replace("while (0)", "while (false)")
        .replace("once", "never");
    let from_test = |l: &&String| l.starts_with("n6");
    assert_eq!(
        drawn(&src, "never")
            .iter()
            .filter(from_test)
            .collect::<Vec<_>>(),
        ["n6{\"while (false)\"}", "n6 -->|done| n7"]
    );
}

#[test]
fn a_do_loop_whose_body_draws_nothing_goes_back_to_its_tests_first_call() {
    let src = "void wait_ready(Port& p) {\n    do {\n        ++tries;\n    } while (!p.ready());\n    done();\n}\n";
    assert_eq!(
        drawn(src, "wait_ready"),
        [
            "n0([\"wait_ready\"])",
            "n1>\"p.ready()\"]:::hidden",
            "n0 --> n1",
            "n2{\"while (!p.ready())\"}",
            "n1 --> n2",
            "n2 -->|loop| n1",
            "n3>\"done()\"]:::hidden",
            "n2 -->|done| n3",
            "n4([\"end\"])",
            "n3 --> n4",
        ]
    );
}

#[test]
fn a_do_loop_whose_body_is_only_a_closure_goes_back_to_its_tests_first_call() {
    let src = "void spin(Port& p) {\n    do {\n        auto f = [&] { poke(); };\n    } while (p.ready());\n}\n";
    let lines = drawn(src, "spin");
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("-->|loop|"))
            .collect::<Vec<_>>(),
        ["n6 -->|loop| n5"]
    );
    assert!(lines.contains(&"n5>\"p.ready()\"]:::hidden".to_string()));
}

#[test]
fn a_do_loop_with_no_call_anywhere_loops_on_its_test() {
    let src = "void count(int n) {\n    do {\n        ++n;\n    } while (n < 9);\n}\n";
    let lines = drawn(src, "count");
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("-->|loop|"))
            .collect::<Vec<_>>(),
        ["n1 -->|loop| n1"]
    );
}
