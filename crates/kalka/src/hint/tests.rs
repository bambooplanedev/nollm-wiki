//! Tests for `hint`.
use super::{
    drop_listed_declarations, is_harness_prompt, page_lines, promote_best_body,
    prompt_from_hook_json,
};
use crate::query::SymbolLine;

fn line(name: &str, start: usize, end: usize, test: bool) -> SymbolLine {
    SymbolLine {
        path: "a.h".into(),
        start_line: start,
        end_line: end,
        name: name.into(),
        signature: String::new(),
        doc: None,
        test,
        nested: false,
        also: None,
        called_by: None,
    }
}

#[test]
fn a_header_declaration_is_dropped_when_its_definition_is_listed() {
    let at = |path: &str, name: &str| {
        let mut s = line(name, 1, 2, false);
        s.path = path.into();
        s
    };
    let (def, decl, other) = (
        at("a.cpp", "A::f"),
        at("a.hpp", "A::f"),
        at("a.hpp", "A::g"),
    );
    let mut rows = vec![
        ("def".to_string(), Some(&def), 0),
        ("decl".to_string(), Some(&decl), 0),
        ("other".to_string(), Some(&other), 0),
    ];
    drop_listed_declarations(&mut rows);
    let kept: Vec<_> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(kept, ["def", "other"]);
}

fn order(scores: &[usize]) -> Vec<String> {
    let mut rows: Vec<_> = scores.iter().map(|&n| (n.to_string(), None, n)).collect();
    promote_best_body(&mut rows);
    rows.into_iter().map(|r| r.0).collect()
}

#[test]
fn the_line_matching_one_and_a_half_times_line_one_goes_first() {
    assert_eq!(order(&[2, 1, 3, 3]), ["3", "2", "1", "3"]);
    assert_eq!(order(&[0, 1]), ["1", "0"]);
}

#[test]
fn a_tie_or_a_smaller_ratio_keeps_the_order() {
    assert_eq!(order(&[3, 4, 3]), ["3", "4", "3"]);
    assert_eq!(order(&[2, 2]), ["2", "2"]);
    assert_eq!(order(&[0, 0]), ["0", "0"]);
    assert_eq!(order(&[]), Vec::<String>::new());
}

#[test]
fn page_lines_drop_tests_and_containers_and_keep_two() {
    let names = |s: &[SymbolLine]| page_lines(s).map(|l| l.name.clone()).collect::<Vec<_>>();
    // The class holds both methods; the test item is dropped.
    let page = [
        line("Fsm", 144, 4487, false),
        line("Fsm::ctor", 157, 212, false),
        line("Fsm::onOrderReject", 1471, 1690, false),
    ];
    assert_eq!(names(&page), ["Fsm::ctor", "Fsm::onOrderReject"]);
    let page = [
        line("decide_test", 640, 650, true),
        line("decide", 76, 107, false),
        line("gate", 200, 220, false),
        line("fetch", 300, 320, false),
    ];
    assert_eq!(names(&page), ["decide", "gate"]);
    // A container whose only inner line is a test item stays.
    let page = [line("mod", 1, 900, false), line("t", 800, 810, true)];
    assert_eq!(names(&page), ["mod"]);
}

#[test]
fn harness_prompts_open_with_a_tag() {
    assert!(is_harness_prompt(
        "<task-notification>\n<task-id>a1</task-id>"
    ));
    assert!(is_harness_prompt("  <bash-stdout>ok</bash-stdout>"));
    assert!(!is_harness_prompt("fix the < comparison in parse_order"));
    assert!(!is_harness_prompt("<3 this parser"));
}

#[test]
fn prompt_from_hook_json_needs_a_string_prompt() {
    assert_eq!(prompt_from_hook_json("{}"), None);
    assert_eq!(prompt_from_hook_json("not json"), None);
    assert_eq!(prompt_from_hook_json(r#"{"prompt": 3}"#), None);
    assert_eq!(
        prompt_from_hook_json(r#"{"prompt": "hi"}"#),
        Some("hi".into())
    );
}
