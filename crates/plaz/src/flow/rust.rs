//! Rust's flow hooks.

use super::{ArmNode, Bindings, FlowNode, Hooks};
use crate::model::{ExitKind, NotDrawn};
use std::collections::BTreeSet;
use wiki_code::support::one_line;
use wiki_code::tree_sitter::Node;

pub(crate) const HOOKS: Hooks = Hooks {
    flow_node,
    bindings,
};

const PANICS: [&str; 5] = ["panic", "unreachable", "todo", "unimplemented", "bail"];
const DEFAULTS: [&str; 5] = [
    "unwrap_or",
    "unwrap_or_else",
    "unwrap_or_default",
    "map_or",
    "map_or_else",
];

const CONTROL: [&str; 8] = [
    "if_expression",
    "match_expression",
    "for_expression",
    "while_expression",
    "loop_expression",
    "return_expression",
    "break_expression",
    "continue_expression",
];

fn text<'t>(n: Node, src: &'t str) -> &'t str {
    &src[n.byte_range()]
}

fn flow_node<'a>(n: Node<'a>, src: &str) -> FlowNode<'a> {
    let field = |f: &str| n.child_by_field_name(f);
    match n.kind() {
        "if_expression" => FlowNode::Branch {
            head: None,
            arms: if_arms(n, src),
            exhaustive: false,
        },
        // `let P = v else { … };`: the pattern is the decision.
        "let_declaration" => match (field("value"), field("alternative")) {
            (Some(value), Some(alt)) => FlowNode::Branch {
                head: None,
                arms: vec![
                    ArmNode {
                        cond: Some(one_line(&src[n.start_byte()..value.end_byte()])),
                        cond_node: Some(value),
                        at: n,
                        body: Vec::new(),
                        after: None,
                    },
                    ArmNode {
                        cond: None,
                        cond_node: None,
                        at: alt,
                        body: vec![alt],
                        after: None,
                    },
                ],
                exhaustive: false,
            },
            _ => FlowNode::Plain,
        },
        "match_expression" => FlowNode::Branch {
            head: field("value").map(|v| format!("match {}", one_line(text(v, src)))),
            arms: field("body")
                .map(|b| {
                    b.named_children(&mut b.walk())
                        .filter(|a| a.kind() == "match_arm")
                        .map(|a| {
                            let pattern = a.child_by_field_name("pattern");
                            ArmNode {
                                cond: pattern.map(|p| one_line(text(p, src))),
                                cond_node: pattern,
                                at: a,
                                body: a.child_by_field_name("value").into_iter().collect(),
                                after: None,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
            exhaustive: true,
        },
        "for_expression" | "while_expression" | "loop_expression" => match field("body") {
            Some(body) => FlowNode::Loop {
                header: one_line(&src[n.start_byte()..body.start_byte()]),
                body,
                each: field("condition").filter(|_| n.kind() == "while_expression"),
                label: label(n, src),
                endless: n.kind() == "loop_expression",
                hidden_exit: hides_break(body, src),
                orelse: None,
            },
            None => FlowNode::Plain,
        },
        "return_expression" => FlowNode::Exit(ExitKind::Return),
        "break_expression" => FlowNode::Exit(ExitKind::Break(label(n, src))),
        "continue_expression" => FlowNode::Exit(ExitKind::Continue(label(n, src))),
        "macro_invocation"
            if field("macro").is_some_and(|m| {
                PANICS.contains(&text(m, src).rsplit("::").next().unwrap_or(""))
            }) && !in_inline_closure(n) =>
        {
            FlowNode::Exit(ExitKind::Return)
        }
        "closure_expression" if own_flow(n) => FlowNode::Nested {
            name: n
                .parent()
                .filter(|p| p.kind() == "let_declaration")
                .and_then(|p| p.child_by_field_name("pattern"))
                .filter(|p| p.kind() == "identifier")
                .map(|p| text(p, src).to_string()),
            body: n,
        },
        "closure_expression" => FlowNode::NotDrawn(NotDrawn::Closure),
        "try_expression" => FlowNode::NotDrawn(NotDrawn::Try),
        // A let chain's `&&` is a token of its own, not a binary expression.
        "&&" if n.parent().is_some_and(|p| p.kind() == "let_chain") => {
            FlowNode::NotDrawn(NotDrawn::ShortCircuit)
        }
        "binary_expression"
            if field("operator").is_some_and(|o| matches!(text(o, src), "&&" | "||")) =>
        {
            FlowNode::NotDrawn(NotDrawn::ShortCircuit)
        }
        "call_expression"
            if field("function")
                .filter(|f| f.kind() == "field_expression")
                .and_then(|f| f.child_by_field_name("field"))
                .is_some_and(|m| DEFAULTS.contains(&text(m, src))) =>
        {
            FlowNode::NotDrawn(NotDrawn::UnwrapOr)
        }
        // Runs later, wherever it is awaited: a flow of its own.
        "async_block" if own_flow(n) => FlowNode::Nested {
            name: Some(format!("async L{}", n.start_position().row + 1)),
            body: n,
        },
        // A function in a body is no item of the file: drawn where it stands.
        "function_item" => FlowNode::Nested {
            name: field("name").map(|i| text(i, src).to_string()),
            body: n,
        },
        _ => FlowNode::Plain,
    }
}

/// Whether a node that `is` is under `n`, not looking into the closures,
/// async blocks and functions under it.
fn holds(n: Node, is: &dyn Fn(Node) -> bool) -> bool {
    let mut stack: Vec<Node> = n.children(&mut n.walk()).collect();
    while let Some(c) = stack.pop() {
        if is(c) {
            return true;
        }
        if !matches!(
            c.kind(),
            "closure_expression" | "async_block" | "function_item"
        ) {
            stack.extend(c.children(&mut c.walk()));
        }
    }
    false
}

/// Whether a macro under the loop body `body`, in no loop inside it, has a
/// `break` token: not the word in a string, a comment or a longer name.
fn hides_break(body: Node, src: &str) -> bool {
    const LOOPS: [&str; 3] = ["for_expression", "while_expression", "loop_expression"];
    holds(body, &|c| {
        c.kind() == "macro_invocation"
            && holds(c, &|t| {
                matches!(t.kind(), "break" | "identifier") && text(t, src) == "break"
            })
            && !std::iter::successors(c.parent(), Node::parent)
                .take_while(|p| *p != body)
                .any(|p| LOOPS.contains(&p.kind()))
    })
}

/// Whether the closure or async block `n` is a flow of its own, not part of
/// the path around it. A closure is one when it has a block body or any
/// control flow: its loops and exits are its own. An async block is one
/// unless it is awaited where it stands and holds no `return`.
fn own_flow(n: Node) -> bool {
    if n.kind() == "async_block" {
        return n.parent().is_none_or(|p| p.kind() != "await_expression")
            || holds(n, &|c| c.kind() == "return_expression");
    }
    n.child_by_field_name("body")
        .is_some_and(|b| b.kind() == "block" || holds(n, &|c| CONTROL.contains(&c.kind())))
}

/// Whether the innermost closure, async block of its own or function around
/// `n` is a closure drawn inline: leaving it leaves nothing drawn.
fn in_inline_closure(n: Node) -> bool {
    let mut at = n.parent();
    while let Some(p) = at {
        match p.kind() {
            "closure_expression" => return !own_flow(p),
            "function_item" => return false,
            "async_block" if own_flow(p) => return false,
            _ => at = p.parent(),
        }
    }
    false
}

/// The `'label` of a loop, `break` or `continue`.
fn label(n: Node, src: &str) -> Option<String> {
    n.children(&mut n.walk())
        .find(|c| c.kind() == "label")
        .map(|c| text(c, src).to_string())
}

/// An `if` and its `else if` chain as arms, the final `else` last.
fn if_arms<'a>(n: Node<'a>, src: &str) -> Vec<ArmNode<'a>> {
    let mut arms = Vec::new();
    let mut at = Some(n);
    while let Some(i) = at.take() {
        let cond = i.child_by_field_name("condition");
        arms.push(ArmNode {
            cond: cond.map(|c| one_line(text(c, src))),
            cond_node: cond,
            at: i,
            body: i.child_by_field_name("consequence").into_iter().collect(),
            after: None,
        });
        let Some(e) = i.child_by_field_name("alternative") else {
            break;
        };
        // A comment between `else` and its block is a named child too.
        match e.named_children(&mut e.walk()).find(|c| !c.is_extra()) {
            Some(next) if next.kind() == "if_expression" => at = Some(next),
            body => arms.push(ArmNode {
                cond: None,
                cond_node: None,
                at: e,
                body: body.into_iter().collect(),
                after: None,
            }),
        }
    }
    arms
}

/// Every name a pattern under `root` binds (parameters, `let`, closure
/// parameters, `for`, `if let`/`while let`, match arms), typed by the last
/// path segment of its annotation or the struct literal it is bound to.
/// Closures and async blocks that are flows of their own, and functions,
/// under `root` bind for their own flows. A unit variant or constant in a match arm (`None`) is
/// indistinguishable from a binding and counts as one. `rebound` holds the
/// generic parameters of `root` and of the items around it, and the names
/// of the items declared under `root`; a binding whose type is one of them
/// has no type.
fn bindings(root: Node, src: &str) -> Bindings {
    let mut binds = Vec::new();
    let mut rebound = generic_params(root, src);
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        let item = n != root && n.kind().ends_with("_item");
        if item {
            rebound.extend(
                n.child_by_field_name("name")
                    .map(|name| text(name, src).to_string()),
            );
        }
        let own = n != root
            && (item || matches!(n.kind(), "async_block" | "closure_expression") && own_flow(n));
        if own {
            continue;
        }
        let field = |f: &str| n.child_by_field_name(f);
        let (pattern, ty) = match n.kind() {
            "parameter" => (field("pattern"), field("type")),
            "let_declaration" => (
                field("pattern"),
                field("type").or_else(|| {
                    field("value")
                        .filter(|v| v.kind() == "struct_expression")
                        .and_then(|v| v.child_by_field_name("name"))
                }),
            ),
            "for_expression" | "let_condition" | "match_arm" => (field("pattern"), None),
            "closure_parameters" => {
                for p in n.named_children(&mut n.walk()) {
                    if p.kind() != "parameter" {
                        names(p, src, &mut |b| binds.push((b, None)));
                    }
                }
                (None, None)
            }
            _ => (None, None),
        };
        if let Some(p) = pattern {
            let ty = ty
                .filter(|_| p.kind() == "identifier")
                .and_then(|t| type_name(t, src));
            names(p, src, &mut |b| binds.push((b, ty.clone())));
        }
        stack.extend(n.named_children(&mut n.walk()));
    }
    let is_rebound = |t: &String| rebound.contains(t.split('<').next().unwrap_or("").trim());
    let binds = binds
        .into_iter()
        .map(|(n, t)| (n, t.filter(|t| !is_rebound(t))));
    let joined = Bindings::join(binds);
    Bindings { rebound, ..joined }
}

/// The type and const parameters of `root` and of each function, impl or
/// trait around it; not lifetimes.
fn generic_params(root: Node, src: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut at = Some(root);
    while let Some(n) = at {
        if matches!(n.kind(), "function_item" | "impl_item" | "trait_item") {
            if let Some(tp) = n.child_by_field_name("type_parameters") {
                for p in tp.named_children(&mut tp.walk()) {
                    if p.kind().contains("lifetime") {
                        continue;
                    }
                    let name = if p.kind() == "type_identifier" {
                        Some(p)
                    } else {
                        p.child_by_field_name("name")
                            .or_else(|| p.child_by_field_name("left"))
                            .or_else(|| {
                                p.named_children(&mut p.walk())
                                    .find(|c| matches!(c.kind(), "type_identifier" | "identifier"))
                            })
                    };
                    out.extend(name.map(|c| text(c, src).to_string()));
                }
            }
        }
        at = n.parent();
    }
    out
}

/// The identifiers a pattern binds: not a tuple-struct or struct pattern's
/// type, not a path, not a match guard.
fn names(p: Node, src: &str, out: &mut impl FnMut(String)) {
    if matches!(p.kind(), "identifier" | "shorthand_field_identifier") {
        out(text(p, src).to_string());
        return;
    }
    let skip = [
        p.child_by_field_name("type"),
        p.child_by_field_name("condition"),
    ];
    for c in p.named_children(&mut p.walk()) {
        if !skip.contains(&Some(c)) && c.kind() != "scoped_identifier" {
            names(c, src, out);
        }
    }
}

/// The type as written, references and `mut` dropped, whitespace
/// collapsed: `Vec<u8>` of `&mut Vec<u8>`, `crate::b::Store`; `None` for a
/// type with no single name (a tuple, `impl Fn`, `dyn Trait`).
fn type_name(t: Node, src: &str) -> Option<String> {
    match t.kind() {
        "type_identifier"
        | "identifier"
        | "generic_type"
        | "generic_type_with_turbofish"
        | "scoped_type_identifier"
        | "scoped_identifier" => Some(one_line(text(t, src))),
        "reference_type" => type_name(t.child_by_field_name("type")?, src),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
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
        assert!(lines.contains(&"nested run @4".to_string()));
        assert!(lines.contains(&"  branch @5".to_string()));
        assert!(lines.contains(&"call run(a) @7".to_string()));
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
        let i = g.code.items.iter().position(|it| it.name == "g");
        assert!(i.is_none_or(|i| g.flows.get(&i).is_none_or(|fl| fl.syntax_errors)));
        assert!(g.flows.values().any(|fl| !fl.syntax_errors)); // `ok` is still charted and clean
    }

    #[test]
    fn two_functions_on_one_line_get_their_own_flows() {
        let f = file("src/a.rs", "fn a() { x(); } fn a2() { if c() { y(); } }\n");
        assert_eq!(
            flow_lines(
                "src/a.rs",
                "fn a() { x(); } fn a2() { if c() { y(); } }\n",
                "a"
            ),
            ["call x() @1"]
        );
        assert_eq!(f.flows.len(), 2);
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
}
