//! Rust's flow hooks.

use super::{text, ArmNode, Bindings, FlowNode, Hooks};
use crate::links::model::{Binding, ExitKind, NotDrawn};
use crate::model::CallVia;
use crate::support::one_line;
use crate::tree_sitter::Node;
use std::collections::BTreeSet;

pub(crate) const HOOKS: Hooks = Hooks {
    flow_node,
    bindings,
    skips_call: |_| false,
    // A statement, a block's tail or a match arm's value, in no closure
    // drawn in line.
    ends_path: |n, _| {
        let tail = n
            .parent()
            .is_some_and(|p| matches!(p.kind(), "block" | "match_arm"));
        (super::whole_statement(n) || tail) && !in_inline_closure(n)
    },
    own_initializer,
    wrapper: |s| s.via == CallVia::Bare && matches!(s.name.as_str(), "Some" | "Ok" | "Err"),
};

/// Is `n` inside the initializer of a `let` binding `name` alone, in the
/// same function? The `let`'s name is not in scope there.
fn own_initializer(n: Node, name: &str, src: &str) -> bool {
    let mut at = n;
    while let Some(p) = at.parent() {
        match p.kind() {
            // A macro may rebind the name where the call is written.
            "function_item" | "token_tree" | "macro_invocation" => return false,
            "let_declaration"
                if p.child_by_field_name("value") == Some(at)
                    && p.child_by_field_name("pattern")
                        .is_some_and(|b| b.kind() == "identifier" && text(b, src) == name) =>
            {
                return true
            }
            _ => at = p,
        }
    }
    false
}

const PANICS: [&str; 5] = ["panic", "unreachable", "todo", "unimplemented", "bail"];
const ASSERTS: [&str; 6] = [
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
];
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

fn flow_node<'a>(n: Node<'a>, src: &str) -> FlowNode<'a> {
    let field = |f: &str| n.child_by_field_name(f);
    match n.kind() {
        "if_expression" => FlowNode::Branch {
            head: None,
            arms: if_arms(n, src),
            exhaustive: false,
            breakable: false,
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
                        falls: false,
                        after: None,
                    },
                    ArmNode {
                        cond: None,
                        cond_node: None,
                        at: alt,
                        body: vec![alt],
                        falls: false,
                        after: None,
                    },
                ],
                exhaustive: false,
                breakable: false,
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
                                falls: false,
                                after: None,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
            exhaustive: true,
            breakable: false,
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
                test_after: false,
                once: false,
                post: None,
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
        "macro_invocation"
            if field("macro").is_some_and(|m| {
                ASSERTS.contains(&text(m, src).rsplit("::").next().unwrap_or(""))
            }) =>
        {
            FlowNode::NotDrawn(NotDrawn::Assert)
        }
        "closure_expression" if own_flow(n) => {
            let bound = n.parent().filter(|p| p.kind() == "let_declaration");
            let name = bound
                .and_then(|p| p.child_by_field_name("pattern"))
                .filter(|p| p.kind() == "identifier");
            FlowNode::Nested {
                name: name.map(|p| text(p, src).to_string()),
                body: n,
                // From the end of the `let` to the end of its block; none for
                // a `let mut` with a declared type, which may be assigned
                // another function (a closure's own type cannot).
                binds: bound
                    .zip(bound.and_then(|b| b.parent()))
                    .filter(|_| name.is_some())
                    .filter(|(l, _)| {
                        let mutable =
                            (l.children(&mut l.walk())).any(|c| c.kind() == "mutable_specifier");
                        !mutable || l.child_by_field_name("type").is_none()
                    })
                    .map(|(l, b)| Binding {
                        scope: l.end_byte()..b.end_byte(),
                        what: "closure",
                    }),
            }
        }
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
            binds: None,
        },
        // A function in a body is no item of the file: drawn where it stands.
        // Its name means it in the whole block it stands in.
        "function_item" => FlowNode::Nested {
            name: field("name").map(|i| text(i, src).to_string()),
            body: n,
            binds: n
                .parent()
                .filter(|_| field("name").is_some())
                .map(|b| Binding {
                    scope: b.byte_range(),
                    what: "nested function",
                }),
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
            falls: false,
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
                falls: false,
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
    // The names `root`'s parameters bind, and those anything else binds.
    let (mut params, mut body) = (BTreeSet::new(), BTreeSet::new());
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
            // A unit variant or constant (`None`) is no binding.
            let unit = matches!(n.kind(), "match_arm" | "let_condition");
            let own_param =
                n.kind() == "parameter" && n.parent().and_then(|p| p.parent()) == Some(root);
            names(p, src, &mut |b| {
                if unit && b.starts_with(char::is_uppercase) {
                    return;
                }
                if own_param {
                    params.insert(b.clone());
                } else {
                    body.insert(b.clone());
                }
                binds.push((b, ty.clone()));
            });
        }
        stack.extend(n.named_children(&mut n.walk()));
    }
    let is_rebound = |t: &String| rebound.contains(t.split('<').next().unwrap_or("").trim());
    let binds = binds
        .into_iter()
        .map(|(n, t)| (n, t.filter(|t| !is_rebound(t))));
    let joined = Bindings::join(binds);
    // `let original = original.trim();` is a local.
    params.retain(|p| !body.contains(p));
    Bindings {
        params,
        rebound,
        ..joined
    }
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
mod tests;
