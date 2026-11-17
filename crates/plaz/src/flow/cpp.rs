//! C and C++'s flow hooks (one grammar for both), and what a file shows
//! outside its functions.

use super::{text, ArmNode, FlowNode, Hooks};
use crate::model::{Bodiless, ExitKind, NotDrawn};
use std::collections::BTreeMap;
use wiki_code::model::{CallSite, CallVia};
use wiki_code::support::one_line;
use wiki_code::tree_sitter::Node;
use wiki_code::Analyzed;

pub(crate) const HOOKS: Hooks = Hooks {
    flow_node,
    bindings: bindings::bindings,
    skips_call: |s: &CallSite| s.via == CallVia::Bare && CASTS.contains(&s.name.as_str()),
};

/// Casts the grammar reads as calls of a template function.
const CASTS: [&str; 4] = [
    "static_cast",
    "dynamic_cast",
    "const_cast",
    "reinterpret_cast",
];

/// Calls that never return.
const ENDS: [&str; 7] = [
    "exit",
    "std::exit",
    "abort",
    "std::abort",
    "std::terminate",
    "_exit",
    "__builtin_unreachable",
];

fn flow_node<'a>(n: Node<'a>, src: &str) -> FlowNode<'a> {
    let field = |f: &str| n.child_by_field_name(f);
    // Asked only of a `try` or `goto` token: finding a parent walks down
    // from the root.
    let parent = || n.parent().map_or("", |p| p.kind());
    match n.kind() {
        "if_statement" => branch(if_arms(n, src), false),
        "preproc_if" | "preproc_ifdef" => {
            let arms = directive_arms(n, src);
            let exhaustive = arms.last().is_some_and(|a| a.cond.is_none());
            branch(arms, exhaustive)
        }
        "switch_statement" => switch(n, src),
        "while_statement" | "for_statement" | "for_range_loop" | "do_statement" => looping(n, src),
        "try_statement" => branch(try_arms(n, src), true),
        // MSVC's `__try` with `__finally` is its body, then the `__finally`.
        "seh_try_statement" if try_arms(n, src).len() > 1 => branch(try_arms(n, src), true),
        "try" if parent() == "try_statement" => FlowNode::NotDrawn(NotDrawn::TryBlock),
        "__try" if parent() == "seh_try_statement" => FlowNode::NotDrawn(NotDrawn::TryBlock),
        "seh_finally_clause" => FlowNode::Finally,
        // A jump to the end of the `__try`, not drawn.
        "seh_leave_statement" => FlowNode::NotDrawn(NotDrawn::TryBlock),
        "return_statement" | "throw_statement" | "co_return_statement" | "goto_statement" => {
            FlowNode::Exit(ExitKind::Return)
        }
        "goto" if parent() == "goto_statement" => FlowNode::NotDrawn(NotDrawn::Goto),
        "break_statement" => FlowNode::Exit(ExitKind::Break(None)),
        "continue_statement" => FlowNode::Exit(ExitKind::Continue(None)),
        // The opening token of a group whose labels `switch` draws as arms:
        // walked once, in an arm.
        "#if" | "#ifdef" | "#ifndef" if n.parent().is_some_and(holds_case) => {
            FlowNode::NotDrawn(NotDrawn::Preprocessor)
        }
        // Every label a switch draws as an arm is an arm's `at`, never walked.
        "case_statement" => FlowNode::NotDrawn(NotDrawn::NestedCase),
        "lambda_expression" if field("body").is_some() => FlowNode::Nested {
            name: n
                .parent()
                .filter(|p| p.kind() == "init_declarator")
                .and_then(|p| p.child_by_field_name("declarator"))
                .filter(|d| d.kind() == "identifier")
                .map(|d| text(d, src).to_string()),
            body: n,
        },
        // A local class's method: no item of the file, drawn where it stands.
        "function_definition" => FlowNode::Nested {
            name: field("declarator")
                .and_then(|d| d.child_by_field_name("declarator"))
                .map(|d| one_line(text(d, src))),
            body: n,
        },
        "conditional_expression" => FlowNode::NotDrawn(NotDrawn::Ternary),
        "binary_expression"
            if field("operator")
                .is_some_and(|o| matches!(text(o, src), "&&" | "||" | "and" | "or")) =>
        {
            FlowNode::NotDrawn(NotDrawn::ShortCircuit)
        }
        "call_expression" => match field("function") {
            Some(f) if ENDS.contains(&text(f, src)) => FlowNode::Exit(ExitKind::Return),
            // A callee with a name has a call site; `float(x)` is a cast.
            Some(f)
                if matches!(
                    f.kind(),
                    "identifier"
                        | "qualified_identifier"
                        | "template_function"
                        | "field_expression"
                        | "primitive_type"
                ) =>
            {
                FlowNode::Plain
            }
            _ => FlowNode::NotDrawn(NotDrawn::Untraced),
        },
        "new_expression" => FlowNode::NotDrawn(NotDrawn::Untraced),
        "expression_statement" if n.named_child_count() == 1 => match n.named_child(0) {
            Some(w) if w.kind() == "identifier" => FlowNode::Word(text(w, src).to_string()),
            _ => FlowNode::Plain,
        },
        "declaration" if swallowed(n, src) => FlowNode::NotDrawn(NotDrawn::Untraced),
        _ => FlowNode::Plain,
    }
}

/// Is the declaration `n` a statement-like macro written without a
/// semicolon and the call after it (`LOCALS` above `CHECK(x);`): its type
/// a lone name in capitals, its one declarator call-shaped and on a later
/// line?
fn swallowed(n: Node, src: &str) -> bool {
    let ty = n.child_by_field_name("type");
    let ty = ty.filter(|t| t.kind() == "type_identifier");
    let caps = ty.is_some_and(|t| {
        let w = text(t, src);
        w.contains(|c: char| c.is_ascii_uppercase())
            && w.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    });
    let mut cursor = n.walk();
    let mut ds = n.children_by_field_name("declarator", &mut cursor);
    let call = match (ds.next(), ds.next(), ty) {
        (Some(d), None, Some(t)) => {
            let value = d.child_by_field_name("value");
            let shaped = d.kind() == "function_declarator"
                || value.is_some_and(|v| v.kind() == "argument_list");
            shaped && d.start_position().row > t.end_position().row
        }
        _ => false,
    };
    caps && call
}

/// `while`, `for` (its update the `post`), range `for`, or `do` (tested
/// after the body); `Plain` without a body.
fn looping<'a>(n: Node<'a>, src: &str) -> FlowNode<'a> {
    let field = |f: &str| n.child_by_field_name(f);
    let (Some(body), cond) = (field("body"), field("condition")) else {
        return FlowNode::Plain;
    };
    let after = n.kind() == "do_statement";
    FlowNode::Loop {
        header: match cond.filter(|_| after) {
            Some(c) => format!("while {}", one_line(text(c, src))),
            None => one_line(&src[n.start_byte()..body.start_byte()]),
        },
        body,
        each: cond,
        label: None,
        endless: match n.kind() {
            "for_statement" => cond.is_none_or(|c| always(c, src)),
            _ => cond.is_some_and(|c| always(c, src)),
        },
        hidden_exit: false,
        orelse: None,
        test_after: after,
        once: after && cond.is_some_and(|c| never(c, src)),
        post: field("update"),
    }
}

fn branch(arms: Vec<ArmNode>, exhaustive: bool) -> FlowNode {
    FlowNode::Branch {
        head: None,
        arms,
        exhaustive,
        breakable: false,
    }
}

/// `true` or a nonzero number, bare or in parentheses: a test that never
/// fails.
fn always(c: Node, src: &str) -> bool {
    let v = literal(c);
    v.kind() == "true" || (v.kind() == "number_literal" && text(v, src) != "0")
}

/// `false` or `0`, bare or in parentheses: a test that never passes.
fn never(c: Node, src: &str) -> bool {
    let v = literal(c);
    v.kind() == "false" || (v.kind() == "number_literal" && text(v, src) == "0")
}

/// The test `c` without its parentheses.
fn literal(c: Node) -> Node {
    let inner = c.child_by_field_name("value").or_else(|| {
        (c.kind() == "parenthesized_expression")
            .then(|| c.named_child(0))
            .flatten()
    });
    inner.unwrap_or(c)
}

/// An `if` and its `else if` chain as arms, the final `else` last; a
/// condition as written inside its parentheses.
fn if_arms<'a>(n: Node<'a>, src: &str) -> Vec<ArmNode<'a>> {
    let mut arms = Vec::new();
    let mut at = Some(n);
    while let Some(i) = at.take() {
        let cond = i.child_by_field_name("condition");
        arms.push(ArmNode {
            cond: cond.map(|c| {
                let t = text(c, src);
                one_line(
                    t.strip_prefix('(')
                        .and_then(|t| t.strip_suffix(')'))
                        .unwrap_or(t),
                )
            }),
            cond_node: cond,
            at: i,
            body: i.child_by_field_name("consequence").into_iter().collect(),
            falls: false,
            after: None,
        });
        let Some(e) = i.child_by_field_name("alternative") else {
            break;
        };
        match e.named_children(&mut e.walk()).find(|c| !c.is_extra()) {
            Some(next) if next.kind() == "if_statement" => at = Some(next),
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

/// A conditional directive group as arms: each `#if…`/`#elif…` line as
/// written, `#else` the else arm.
fn directive_arms<'a>(n: Node<'a>, src: &str) -> Vec<ArmNode<'a>> {
    let mut arms = Vec::new();
    let mut at = Some(n);
    while let Some(g) = at.take() {
        let cond = g
            .child_by_field_name("condition")
            .or_else(|| g.child_by_field_name("name"));
        let alt = g.child_by_field_name("alternative");
        let line_end = cond
            .or_else(|| g.child(0))
            .map_or(g.start_byte(), |c| c.end_byte());
        arms.push(ArmNode {
            cond: (g.kind() != "preproc_else").then(|| one_line(&src[g.start_byte()..line_end])),
            cond_node: cond,
            at: g,
            body: g
                .named_children(&mut g.walk())
                .filter(|&c| Some(c) != cond && Some(c) != alt)
                .collect(),
            falls: false,
            after: None,
        });
        at = alt;
    }
    arms
}

/// Whether a `case` label stands in the directive group `g`, in any of
/// its branches or of the groups nested in them.
fn holds_case(g: Node) -> bool {
    g.named_children(&mut g.walk()).any(|c| {
        c.kind() == "case_statement" || (c.kind().starts_with("preproc_") && holds_case(c))
    })
}

/// `switch (…)` and an arm per run of stacked labels, the labels as
/// written, each arm falling into the next.
fn switch<'a>(n: Node<'a>, src: &str) -> FlowNode<'a> {
    let body = n.child_by_field_name("body");
    let mut entries = Vec::new();
    if let Some(b) = body {
        flatten(b.named_children(&mut b.walk()), src, &mut entries);
    }
    // Per arm: its labels, its body, and whether the body has a statement
    // yet; a label before any statement stacks onto the arm.
    let mut runs: Vec<(Vec<Node>, Vec<Node>, bool)> = Vec::new();
    // Group tokens before the first label; statements there run only by a
    // `goto`, counted by `before_case`.
    let mut early = Vec::new();
    for c in entries {
        let label = c.kind() == "case_statement";
        match runs.last_mut() {
            Some((labels, _, false)) if label => labels.push(c),
            _ if label => runs.push((vec![c], std::mem::take(&mut early), false)),
            Some((_, body, stated)) => {
                *stated |= c.is_named();
                body.push(c);
            }
            None => early.extend((!c.is_named()).then_some(c)),
        }
    }
    let exhaustive = runs
        .iter()
        .flat_map(|r| &r.0)
        .any(|l| l.child_by_field_name("value").is_none());
    let label = |l: &Node| {
        let colon = l.children(&mut l.walk()).find(|c| c.kind() == ":");
        one_line(&src[l.start_byte()..colon.unwrap_or(*l).end_byte()])
    };
    let last = runs.len().saturating_sub(1);
    let arms = runs
        .into_iter()
        .enumerate()
        .map(|(i, (labels, body, _))| ArmNode {
            cond: Some(labels.iter().map(label).collect::<Vec<_>>().join(" ")),
            cond_node: None,
            at: labels[0],
            body,
            falls: i < last,
            after: None,
        })
        .collect();
    FlowNode::Branch {
        head: body.map(|b| one_line(&src[n.start_byte()..b.start_byte()])),
        arms,
        exhaustive,
        breakable: true,
    }
}

/// A switch body's labels and statements in source order: each `case`
/// node, then its statements; a directive group holding labels opened up
/// into its branches, its opening token first; comments left out.
fn flatten<'a>(nodes: impl Iterator<Item = Node<'a>>, src: &str, out: &mut Vec<Node<'a>>) {
    for c in nodes {
        match c.kind() {
            "comment" => {}
            "case_statement" => {
                out.push(c);
                let value = c.child_by_field_name("value");
                out.extend(
                    c.named_children(&mut c.walk())
                        .filter(|&s| Some(s) != value && s.kind() != "comment"),
                );
            }
            "preproc_if" | "preproc_ifdef" if holds_case(c) => {
                out.extend(c.child(0));
                for arm in directive_arms(c, src) {
                    flatten(arm.body.into_iter(), src, out);
                }
            }
            _ => out.push(c),
        }
    }
}

/// The statements of each switch under `n` that come before its first
/// label: only a `goto` reaches them, and no arm draws them.
pub(super) fn before_case(n: Node, src: &str) -> usize {
    let mut count = 0;
    let mut stack = vec![n];
    while let Some(c) = stack.pop() {
        if let Some(b) = c
            .child_by_field_name("body")
            .filter(|_| c.kind() == "switch_statement")
        {
            let mut entries = Vec::new();
            flatten(b.named_children(&mut b.walk()), src, &mut entries);
            count += (entries.iter())
                .take_while(|e| e.kind() != "case_statement")
                .filter(|e| e.is_named())
                .count();
        }
        stack.extend(c.named_children(&mut c.walk()));
    }
    count
}

/// What a function item's node has in place of a body to chart; `None`
/// for a definition with a block (or a function-try-block) body.
pub(super) fn bodiless(node: Node) -> Option<Bodiless> {
    if node.kind() != "function_definition" {
        return Some(Bodiless::Declared);
    }
    let body = node.child_by_field_name("body").map(|b| b.kind());
    if matches!(body, Some("compound_statement" | "try_statement")) {
        return None;
    }
    let clause = |k: &str| node.named_children(&mut node.walk()).any(|c| c.kind() == k);
    Some(if clause("default_method_clause") {
        Bodiless::Defaulted
    } else if clause("delete_method_clause") {
        Bodiless::Deleted
    } else {
        Bodiless::Declared
    })
}

/// The body as the arm `try`, each `catch (…)` or `__except (…)` as an arm.
fn try_arms<'a>(n: Node<'a>, src: &str) -> Vec<ArmNode<'a>> {
    let mut arms = vec![ArmNode {
        cond: Some("try".to_string()),
        cond_node: None,
        at: n,
        body: n.child_by_field_name("body").into_iter().collect(),
        falls: false,
        after: None,
    }];
    for h in n.named_children(&mut n.walk()) {
        if !matches!(h.kind(), "catch_clause" | "seh_except_clause") {
            continue;
        }
        let end = h
            .child_by_field_name("parameters")
            .or_else(|| h.child_by_field_name("filter"))
            .or_else(|| h.child(0))
            .map_or(h.start_byte(), |p| p.end_byte());
        arms.push(ArmNode {
            cond: Some(one_line(&src[h.start_byte()..end])),
            cond_node: None,
            at: h,
            body: h.child_by_field_name("body").into_iter().collect(),
            falls: false,
            after: None,
        });
    }
    arms
}

/// Does a declaration outside any function initialize with a call? That
/// call runs before `main`, in no flow.
pub(crate) fn module_code(a: &Analyzed) -> bool {
    a.sources.iter().any(|s| {
        let mut stack = vec![s.tree().root_node()];
        while let Some(n) = stack.pop() {
            match n.kind() {
                "function_definition" => {}
                "declaration" if initializes_with_call(n, s.text()) => return true,
                _ => stack.extend(n.named_children(&mut n.walk())),
            }
        }
        false
    })
}

/// A call in the initializers of the declaration `n`, outside any lambda;
/// a cast is none.
fn initializes_with_call(n: Node, src: &str) -> bool {
    let mut stack: Vec<Node> = n
        .children_by_field_name("declarator", &mut n.walk())
        .filter(|d| d.kind() == "init_declarator")
        .collect();
    while let Some(c) = stack.pop() {
        let cast = || {
            c.child_by_field_name("function")
                .filter(|f| f.kind() == "template_function")
                .and_then(|f| f.child_by_field_name("name"))
                .is_some_and(|name| CASTS.contains(&text(name, src)))
        };
        match c.kind() {
            "lambda_expression" => {}
            "call_expression" if !cast() => return true,
            _ => stack.extend(c.named_children(&mut c.walk())),
        }
    }
    false
}

/// Every `#define` name of the file in either tree, once, sorted, and
/// whether a body of it holds a jump keyword as a token outside comments
/// and string literals.
pub(crate) fn macros(a: &Analyzed) -> Vec<(String, bool)> {
    let mut out: BTreeMap<String, bool> = BTreeMap::new();
    for s in &a.sources {
        let mut stack = vec![s.tree().root_node()];
        while let Some(n) = stack.pop() {
            let define = matches!(n.kind(), "preproc_def" | "preproc_function_def");
            if let Some(name) = n.child_by_field_name("name").filter(|_| define) {
                let head = n.child_by_field_name("parameters").unwrap_or(name);
                let body = &s.text()[head.end_byte()..n.end_byte()];
                *out.entry(text(name, s.text()).to_string()).or_default() |= jumps(body);
            }
            stack.extend(n.named_children(&mut n.walk()));
        }
    }
    out.into_iter().collect()
}

/// Whether a macro body holds `return`, `goto`, `break`, `continue` or
/// `throw` as a token outside comments, string and character literals.
fn jumps(body: &str) -> bool {
    let mut code = String::new();
    let mut rest = body;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("//") {
            break;
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r.split_once("*/").map_or("", |(_, after)| after);
        } else if c == '"' || (c == '\'' && !code.ends_with(|d: char| d.is_ascii_digit())) {
            // A quote after a digit is a digit separator (`1'000`).
            let r = &rest[1..];
            let mut chars = r.char_indices();
            let mut end = r.len();
            while let Some((i, ch)) = chars.next() {
                match ch {
                    '\\' => {
                        chars.next();
                    }
                    q if q == c => {
                        end = i + 1;
                        break;
                    }
                    _ => {}
                }
            }
            rest = &r[end..];
        } else {
            code.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        code.push(' ');
    }
    code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| matches!(w, "return" | "goto" | "break" | "continue" | "throw"))
}

mod bindings;
mod groups;
pub(crate) use bindings::fields;
pub(super) use groups::{directive_groups, holding, Group};
#[cfg(test)]
mod tests;
