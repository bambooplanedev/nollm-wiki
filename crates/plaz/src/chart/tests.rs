use super::*;
use crate::flow::testutil::{arm, branch, call, exit, flow_of, looping};
use crate::model::NotDrawn;

/// The node and edge lines of the chart of `steps`.
fn drawn(steps: Vec<Step>) -> Vec<String> {
    chart("f", "a.c", &flow_of(steps), &|_| String::new())
        .lines()
        .filter(|l| l.starts_with("    n"))
        .map(|l| l.trim().to_string())
        .collect()
}

fn a_breakable_k(inside: Vec<Step>) -> Step {
    looping(
        "go",
        vec![
            branch(
                vec![
                    arm(Some("k"), inside, false),
                    arm(None, vec![call("b")], false),
                ],
                true,
            ),
            call("c"),
        ],
        Vec::new(),
        false,
    )
}

#[test]
fn a_break_in_a_breakable_branch_lands_on_what_follows_the_branch() {
    let steps = vec![a_breakable_k(vec![call("a"), exit(ExitKind::Break(None))])];
    assert_eq!(
        drawn(steps),
        [
            "n0([\"f\"])",
            "n1{\"go\"}",
            "n0 --> n1",
            "n2{\"k\"}",
            "n1 -->|loop| n2",
            "n3>\"a()\"]:::hidden",
            "n2 -->|yes| n3",
            "n4[\"exit\"]",
            "n3 --> n4",
            "n5>\"b()\"]:::hidden",
            "n2 -->|no| n5",
            "n6>\"c()\"]:::hidden",
            "n5 --> n6",
            "n4 --> n6",
            "n6 --> n1",
            "n7([\"end\"])",
            "n1 -->|done| n7",
        ]
    );
}

#[test]
fn a_continue_in_a_breakable_branch_goes_to_the_loop() {
    let steps = vec![a_breakable_k(vec![
        call("a"),
        exit(ExitKind::Continue(None)),
    ])];
    assert_eq!(
        drawn(steps),
        [
            "n0([\"f\"])",
            "n1{\"go\"}",
            "n0 --> n1",
            "n2{\"k\"}",
            "n1 -->|loop| n2",
            "n3>\"a()\"]:::hidden",
            "n2 -->|yes| n3",
            "n4[\"exit\"]",
            "n3 --> n4",
            "n4 --> n1",
            "n5>\"b()\"]:::hidden",
            "n2 -->|no| n5",
            "n6>\"c()\"]:::hidden",
            "n5 --> n6",
            "n6 --> n1",
            "n7([\"end\"])",
            "n1 -->|done| n7",
        ]
    );
}

#[test]
fn a_break_in_a_loop_inside_a_breakable_branch_leaves_that_loop() {
    let inner = looping(
        "again",
        vec![exit(ExitKind::Break(None))],
        Vec::new(),
        false,
    );
    let steps = vec![
        branch(vec![arm(Some("k"), vec![inner, call("w")], false)], true),
        call("c"),
    ];
    assert_eq!(
        drawn(steps),
        [
            "n0([\"f\"])",
            "n1{\"k\"}",
            "n0 --> n1",
            "n2{\"again\"}",
            "n1 -->|yes| n2",
            "n3[\"exit\"]",
            "n2 -->|loop| n3",
            "n4>\"w()\"]:::hidden",
            "n3 --> n4",
            "n2 -->|done| n4",
            "n5>\"c()\"]:::hidden",
            "n4 --> n5",
            "n1 -->|no| n5",
            "n6([\"end\"])",
            "n5 --> n6",
        ]
    );
}

#[test]
fn a_labelled_break_passes_through_a_breakable_branch() {
    let mut outer = a_breakable_k(vec![exit(ExitKind::Break(Some("outer".into())))]);
    if let Step::Loop { label, .. } = &mut outer {
        *label = Some("outer".into());
    }
    assert_eq!(
        drawn(vec![outer]),
        [
            "n0([\"f\"])",
            "n1{\"go\"}",
            "n0 --> n1",
            "n2{\"k\"}",
            "n1 -->|loop| n2",
            "n3[\"exit\"]",
            "n2 -->|yes| n3",
            "n4>\"b()\"]:::hidden",
            "n2 -->|no| n4",
            "n5>\"c()\"]:::hidden",
            "n4 --> n5",
            "n5 --> n1",
            "n6([\"end\"])",
            "n3 --> n6",
            "n1 -->|done| n6",
        ]
    );
}

#[test]
fn a_falling_arm_joins_the_next_arms_body_and_not_the_join() {
    let steps = vec![
        branch(
            vec![
                arm(Some("1"), vec![call("a")], true),
                arm(Some("2"), vec![call("b")], true),
            ],
            false,
        ),
        call("c"),
    ];
    assert_eq!(
        drawn(steps),
        [
            "n0([\"f\"])",
            "n1{\"1\"}",
            "n0 --> n1",
            "n2>\"a()\"]:::hidden",
            "n1 -->|yes| n2",
            "n3{\"2\"}",
            "n1 -->|no| n3",
            "n4>\"b()\"]:::hidden",
            "n3 -->|yes| n4",
            "n2 --> n4",
            "n5>\"c()\"]:::hidden",
            "n4 --> n5",
            "n3 -->|no| n5",
            "n6([\"end\"])",
            "n5 --> n6",
        ]
    );
}

#[test]
fn a_test_after_loop_enters_its_body_and_tests_after_it() {
    let body = vec![
        call("a"),
        branch(
            vec![arm(Some("k"), vec![exit(ExitKind::Continue(None))], false)],
            false,
        ),
        call("b"),
    ];
    let steps = vec![looping("go", body, Vec::new(), true)];
    assert_eq!(
        drawn(steps),
        [
            "n0([\"f\"])",
            "n1>\"a()\"]:::hidden",
            "n0 --> n1",
            "n2{\"k\"}",
            "n1 --> n2",
            "n3[\"exit\"]",
            "n2 -->|yes| n3",
            "n4>\"b()\"]:::hidden",
            "n2 -->|no| n4",
            "n5{\"go\"}",
            "n4 --> n5",
            "n3 --> n5",
            "n5 -->|loop| n1",
            "n6([\"end\"])",
            "n5 -->|done| n6",
        ]
    );
}

#[test]
fn a_post_runs_after_the_body_and_a_continue_before_the_header() {
    let body = vec![
        call("a"),
        branch(
            vec![arm(Some("k"), vec![exit(ExitKind::Continue(None))], false)],
            false,
        ),
        call("b"),
    ];
    let steps = vec![looping("go", body, vec![call("u")], false)];
    assert_eq!(
        drawn(steps),
        [
            "n0([\"f\"])",
            "n1{\"go\"}",
            "n0 --> n1",
            "n2>\"a()\"]:::hidden",
            "n1 -->|loop| n2",
            "n3{\"k\"}",
            "n2 --> n3",
            "n4[\"exit\"]",
            "n3 -->|yes| n4",
            "n5>\"b()\"]:::hidden",
            "n3 -->|no| n5",
            "n6>\"u()\"]:::hidden",
            "n5 --> n6",
            "n4 --> n6",
            "n6 --> n1",
            "n7([\"end\"])",
            "n1 -->|done| n7",
        ]
    );
}

#[test]
fn calls_and_closures_in_a_post_are_counted() {
    let mut nested = flow_of(vec![call("n")]);
    nested.not_drawn.insert(NotDrawn::Closure, 1);
    let post = vec![
        call("u"),
        Step::Nested {
            name: "g".into(),
            line: 1,
            flow: nested,
            at: 0..0,
            binds: None,
        },
    ];
    let flow = flow_of(vec![looping("go", vec![call("a")], post, false)]);
    assert_eq!(
        coverage(&flow),
        "Coverage: 3 calls: 0 resolved, 0 one of, 0 outside, 3 hidden. Not drawn: 1 closure."
    );
}

#[test]
fn relative_page_ids_climb_only_past_the_shared_directories() {
    assert_eq!(relative("map/src/x/a_rs", "map/src/b_rs"), "../b_rs");
    assert_eq!(relative("map/src/a_rs", "map/src/a_rs"), "a_rs");
    assert_eq!(relative("map/a_rs", "map/src/x/b_rs"), "src/x/b_rs");
    assert_eq!(relative("map/x/a_rs", "map/y/x"), "../y/x");
}
