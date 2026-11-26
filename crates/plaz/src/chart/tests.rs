use super::*;
use crate::flow::testutil::{arm, branch, call, exit, flow_of, looping};
use crate::model::NotDrawn;
use wiki_code::model::CallVia;

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

/// A call `text` at `bytes`, reaching `target` through `via`.
fn at(text: &str, bytes: std::ops::Range<usize>, target: Target, via: CallVia) -> Step {
    let Step::Call(mut c) = call(text) else {
        unreachable!()
    };
    c.text = text.to_string();
    (c.bytes, c.target, c.site.via) = (bytes, target, via);
    Step::Call(c)
}

#[test]
fn a_hidden_chain_step_is_not_drawn_only_on_an_outside_receiver() {
    let member = || CallVia::Member(None);
    // `h(1)` is the hidden head of the chain: drawn. `h(1).a()` is outside,
    // `h(1).a().b()` hidden on it, both steps of `h(1).a().b().c()`.
    let steps = vec![
        at("h(1)", 0..4, Target::Hidden, CallVia::Bare),
        at("h(1).a()", 0..8, Target::Outside, member()),
        at("h(1).a().b()", 0..12, Target::Hidden, member()),
        at("h(1).a().b().c()", 0..16, Target::Outside, member()),
    ];
    let lines = drawn(steps.clone());
    assert!(
        lines.contains(&"n1>\"h(1)\"]:::hidden".to_string()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"n2[/\"h(1).a().b().c()\"/]".to_string()),
        "{lines:?}"
    );
    assert_eq!(lines.iter().filter(|l| !l.contains("-->")).count(), 4);
    assert_eq!(
        coverage(&flow_of(steps)),
        "Coverage: 4 calls: 0 resolved, 0 one of, 2 outside, 2 hidden. Not drawn: 2 chain step."
    );
}

fn outside(text: &str) -> Step {
    at(text, 0..0, Target::Outside, CallVia::Bare)
}

#[test]
fn a_run_of_calls_of_one_unlinked_form_is_one_node() {
    let sure = Target::Resolved(FnId {
        path: "a.rs".into(),
        item: 0,
    });
    let steps = vec![
        outside("a()"),
        outside("b()"),
        // A certain call breaks the run, as does a call of another form.
        at("c()", 0..0, sure, CallVia::Bare),
        outside("d()"),
        outside("e()"),
        call("h"),
        call("i"),
        // A decision breaks it; so does the join after it.
        branch(
            vec![arm(Some("k"), vec![outside("f()"), outside("g()")], false)],
            false,
        ),
        outside("x()"),
        outside("y()"),
    ];
    assert_eq!(
        drawn(steps),
        [
            "n0([\"f\"])",
            "n1[/\"a()<br/>b()\"/]",
            "n0 --> n1",
            "n2[\"c()\"]",
            "n1 --> n2",
            "n3[/\"d()<br/>e()\"/]",
            "n2 --> n3",
            "n4>\"h()<br/>i()\"]:::hidden",
            "n3 --> n4",
            "n5{\"k\"}",
            "n4 --> n5",
            "n6[/\"f()<br/>g()\"/]",
            "n5 -->|yes| n6",
            "n7[/\"x()<br/>y()\"/]",
            "n6 --> n7",
            "n5 -->|no| n7",
            "n8([\"end\"])",
            "n7 --> n8",
        ]
    );
}

#[test]
fn a_run_longer_than_eight_calls_is_split() {
    let steps = (0..10).map(|i| outside(&format!("s{i}()"))).collect();
    let lines = drawn(steps);
    assert_eq!(
        lines[1],
        "n1[/\"s0()<br/>s1()<br/>s2()<br/>s3()<br/>s4()<br/>s5()<br/>s6()<br/>s7()\"/]"
    );
    assert_eq!(lines[3], "n2[/\"s8()<br/>s9()\"/]");
}
