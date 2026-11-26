//! Python's flow hooks, and the imports a module's tree shows.

use super::{text, ArmNode, Bindings, FlowNode, Hooks};
use crate::model::{Binding, ExitKind, Import, NotDrawn};
use std::collections::BTreeSet;
use wiki_code::support::one_line;
use wiki_code::tree_sitter::Node;
use wiki_code::Analyzed;

pub(crate) const HOOKS: Hooks = Hooks {
    flow_node,
    bindings,
    skips_call: |_| false,
    ends_path: |n, src| super::whole_statement(n) && !caught_exit(n, src),
    // `x = x()` makes `x` local to the whole function: the call fails.
    own_initializer: |_, _, _| false,
    wrapper: |_| false,
};

/// Is `n` in the body of a `try` whose handler catches an exit: a bare
/// `except:`, or one naming `SystemExit` or `BaseException`?
fn caught_exit(n: Node, src: &str) -> bool {
    let mut at = n;
    while let Some(p) = at.parent() {
        if matches!(p.kind(), "function_definition" | "lambda") {
            return false;
        }
        let in_body = p.child_by_field_name("body") == Some(at);
        if p.kind() == "try_statement" && in_body {
            let catches = |h: &Node| {
                let body = h
                    .named_children(&mut h.walk())
                    .find(|c| c.kind() == "block");
                let head = &src[h.start_byte()..body.map_or(h.end_byte(), |b| b.start_byte())];
                let mut words = head.split(|c: char| !(c.is_alphanumeric() || c == '_'));
                head.trim_end().trim_end_matches(':').trim() == "except"
                    || words.any(|w| matches!(w, "SystemExit" | "BaseException"))
            };
            if handlers(p).iter().any(catches) {
                return true;
            }
        }
        at = p;
    }
    false
}

fn flow_node<'a>(n: Node<'a>, src: &str) -> FlowNode<'a> {
    let field = |f: &str| n.child_by_field_name(f);
    let parent = n.parent().map_or("", |p| p.kind());
    match n.kind() {
        "if_statement" => FlowNode::Branch {
            head: None,
            arms: if_arms(n, src),
            exhaustive: false,
            breakable: false,
        },
        "match_statement" => match_branch(n, src),
        "for_statement" | "while_statement" => match field("body") {
            Some(body) => {
                let cond = field("condition");
                let header_end = field("right")
                    .or(cond)
                    .map_or(body.start_byte(), |h| h.end_byte());
                FlowNode::Loop {
                    header: one_line(&src[n.start_byte()..header_end]),
                    body,
                    each: cond,
                    label: None,
                    // `while True:` is left only by a `break`.
                    endless: cond.is_some_and(|c| {
                        c.kind() == "true" || (c.kind() == "integer" && text(c, src) != "0")
                    }),
                    hidden_exit: false,
                    orelse: field("alternative"),
                    test_after: false,
                    once: false,
                    post: None,
                }
            }
            None => FlowNode::Plain,
        },
        // With handlers, a decision between the body and each handler; the
        // edges from inside the body to a handler are not drawn.
        "try_statement" if !handlers(n).is_empty() => FlowNode::Branch {
            head: None,
            arms: try_arms(n, src),
            exhaustive: true,
            breakable: false,
        },
        "try" if parent == "try_statement" => FlowNode::NotDrawn(NotDrawn::TryBlock),
        "finally_clause" => FlowNode::Finally,
        "return_statement" | "raise_statement" => FlowNode::Exit(ExitKind::Return),
        "break_statement" => FlowNode::Exit(ExitKind::Break(None)),
        "continue_statement" => FlowNode::Exit(ExitKind::Continue(None)),
        // A def in a body is no item of the file: drawn where it stands.
        "function_definition" => FlowNode::Nested {
            name: field("name").map(|i| text(i, src).to_string()),
            body: n,
            binds: def_scope(n).map(|scope| Binding {
                scope,
                what: "nested function",
            }),
        },
        // Expressions: they hold no statements, so they are drawn inline.
        "lambda"
        | "list_comprehension"
        | "dictionary_comprehension"
        | "set_comprehension"
        | "generator_expression"
            if n.is_named() =>
        {
            FlowNode::NotDrawn(NotDrawn::Closure)
        }
        "conditional_expression" => FlowNode::NotDrawn(NotDrawn::Ternary),
        "boolean_operator" => FlowNode::NotDrawn(NotDrawn::ShortCircuit),
        "assert_statement" => FlowNode::NotDrawn(NotDrawn::Assert),
        _ => FlowNode::Plain,
    }
}

/// Where the name of the nested def `n` means it: from the def to the end
/// of the function around it. None for a decorated def, whose name holds
/// what the decorator returns, and for a method of a class in the body.
fn def_scope(n: Node) -> Option<std::ops::Range<usize>> {
    let mut at = n.parent()?;
    loop {
        match at.kind() {
            "decorated_definition" | "class_definition" => return None,
            "function_definition" => return Some(n.start_byte()..at.end_byte()),
            _ => at = at.parent()?,
        }
    }
}

/// An `if` and its `elif`s as arms, the `else` last.
fn if_arms<'a>(n: Node<'a>, src: &str) -> Vec<ArmNode<'a>> {
    let arm = |at: Node<'a>| {
        let cond = at.child_by_field_name("condition");
        ArmNode {
            cond: cond.map(|c| one_line(text(c, src))),
            cond_node: cond,
            at,
            body: at.child_by_field_name("consequence").into_iter().collect(),
            falls: false,
            after: None,
        }
    };
    let mut arms = vec![arm(n)];
    for alt in n.children_by_field_name("alternative", &mut n.walk()) {
        arms.push(if alt.kind() == "elif_clause" {
            arm(alt)
        } else {
            ArmNode {
                cond: None,
                cond_node: None,
                at: alt,
                body: alt.child_by_field_name("body").into_iter().collect(),
                falls: false,
                after: None,
            }
        });
    }
    arms
}

/// `match <subject>` and an arm per `case`: its patterns and guard as
/// written. No way past the last case when it is `_` or a bare name.
fn match_branch<'a>(n: Node<'a>, src: &str) -> FlowNode<'a> {
    let subjects: Vec<Node> = n.children_by_field_name("subject", &mut n.walk()).collect();
    let head = subjects
        .first()
        .zip(subjects.last())
        .map(|(a, b)| format!("match {}", one_line(&src[a.start_byte()..b.end_byte()])));
    let cases: Vec<Node> = n.child_by_field_name("body").map_or(Vec::new(), |b| {
        b.named_children(&mut b.walk())
            .filter(|c| c.kind() == "case_clause")
            .collect()
    });
    let mut exhaustive = false;
    let arms = cases
        .iter()
        .map(|&c| {
            let patterns: Vec<Node> = c
                .named_children(&mut c.walk())
                .filter(|p| p.kind() == "case_pattern")
                .collect();
            let guard = c.child_by_field_name("guard");
            let span = patterns.first().map(|f| {
                let end = guard
                    .or(patterns.last().copied())
                    .map_or(f.end_byte(), |e| e.end_byte());
                &src[f.start_byte()..end]
            });
            // `_` or a capture name; `None`, `True` and `False` are values.
            exhaustive = guard.is_none()
                && patterns.len() == 1
                && span.is_some_and(|p| {
                    p.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
                        && !p.starts_with(|ch: char| ch.is_ascii_digit())
                        && !matches!(p, "None" | "True" | "False")
                });
            ArmNode {
                cond: span.map(one_line),
                cond_node: guard,
                at: c,
                body: c.child_by_field_name("consequence").into_iter().collect(),
                falls: false,
                after: None,
            }
        })
        .collect();
    FlowNode::Branch {
        head,
        arms,
        exhaustive,
        breakable: false,
    }
}

fn handlers(n: Node) -> Vec<Node> {
    n.named_children(&mut n.walk())
        .filter(|c| c.kind() == "except_clause")
        .collect()
}

/// The body as the arm `try`, each handler as the arm `except …`.
fn try_arms<'a>(n: Node<'a>, src: &str) -> Vec<ArmNode<'a>> {
    let mut arms = vec![ArmNode {
        cond: Some("try".to_string()),
        cond_node: None,
        at: n,
        body: n.child_by_field_name("body").into_iter().collect(),
        falls: false,
        // The `else` runs after the body only.
        after: n
            .named_children(&mut n.walk())
            .find(|c| c.kind() == "else_clause"),
    }];
    for h in handlers(n) {
        let parts: Vec<Node> = h.named_children(&mut h.walk()).collect();
        let body = parts.iter().copied().find(|c| c.kind() == "block");
        let end = parts
            .iter()
            .filter(|c| c.kind() != "block" && !c.is_extra())
            .map(Node::end_byte)
            .max()
            .unwrap_or(h.start_byte() + "except".len());
        arms.push(ArmNode {
            cond: Some(one_line(&src[h.start_byte()..end])),
            cond_node: h.child_by_field_name("value"),
            at: h,
            body: body.into_iter().collect(),
            falls: false,
            after: None,
        });
    }
    arms
}

/// Every name the function `root` binds: parameters (typed by their
/// annotation), assignment, `for`, `with … as`, `except … as`, walrus,
/// comprehension, `case` pattern, `global` and `nonlocal` names; `x = T()`
/// with `T` capitalized types `x`. The defs, classes and imports of the
/// body are `rebound`; the bodies of defs and classes bind for themselves.
fn bindings(root: Node, src: &str) -> Bindings {
    let mut binds: Vec<(String, Option<String>)> = Vec::new();
    let mut rebound = BTreeSet::new();
    let (mut params, mut outside) = (BTreeSet::new(), BTreeSet::new());
    let mut stack: Vec<Node> = root.children(&mut root.walk()).collect();
    while let Some(n) = stack.pop() {
        let field = |f: &str| n.child_by_field_name(f);
        let mut bind = |p: Option<Node>, ty: Option<String>| {
            if let Some(p) = p {
                names(p, src, &mut |b| binds.push((b, ty.clone())));
            }
        };
        match n.kind() {
            "function_definition" | "class_definition" => {
                rebound.extend(field("name").map(|i| text(i, src).to_string()));
                continue;
            }
            "import_statement" | "import_from_statement" => {
                rebound.extend(
                    statement_imports(n, src)
                        .iter()
                        .filter_map(|i| i.binds())
                        .map(String::from),
                );
                continue;
            }
            "parameters" | "lambda_parameters" => {
                for p in n.named_children(&mut n.walk()) {
                    let ty = p.child_by_field_name("type");
                    let name = p
                        .child_by_field_name("name")
                        .or_else(|| p.named_children(&mut p.walk()).find(|c| Some(*c) != ty));
                    let name = if p.kind() == "identifier" {
                        Some(p)
                    } else {
                        name
                    };
                    if let Some(own) = name.filter(|_| n.parent() == Some(root)) {
                        names(own, src, &mut |b| {
                            params.insert(b);
                        });
                    }
                    bind(name, ty.and_then(|t| type_name(t, src)));
                }
            }
            "assignment" => {
                let left = field("left");
                let ty = field("type").and_then(|t| type_name(t, src)).or_else(|| {
                    field("right")
                        .filter(|r| r.kind() == "call")
                        .and_then(|r| r.child_by_field_name("function"))
                        .filter(|f| f.kind() == "identifier")
                        .map(|f| text(f, src))
                        .filter(|f| f.starts_with(|c: char| c.is_uppercase()))
                        .map(String::from)
                });
                let ty = ty.filter(|_| left.is_some_and(|l| l.kind() == "identifier"));
                bind(left, ty);
            }
            "augmented_assignment" | "for_statement" | "for_in_clause" => bind(field("left"), None),
            "as_pattern" | "except_clause" => bind(field("alias"), None),
            "named_expression" => bind(field("name"), None),
            "global_statement" | "nonlocal_statement" => {
                names(n, src, &mut |b| {
                    outside.insert(b);
                });
                bind(Some(n), None);
            }
            "case_pattern" => captures(n, src, &mut |b| binds.push((b, None))),
            _ => {}
        }
        stack.extend(n.named_children(&mut n.walk()));
    }
    let binds = binds.into_iter().map(|(n, t)| {
        (
            n,
            t.filter(|t| !rebound.contains(t.split("::").next().unwrap_or(""))),
        )
    });
    let joined = Bindings::join(binds);
    Bindings {
        params,
        outside,
        rebound,
        ..joined
    }
}

/// The names a `case` pattern binds: not a class pattern's head
/// (`str()`), a dotted value (`Color.RED`) or a keyword (`x=` of
/// `Point(x=0)`).
fn captures(p: Node, src: &str, out: &mut impl FnMut(String)) {
    let kids: Vec<Node> = p.named_children(&mut p.walk()).collect();
    let skip = match p.kind() {
        "identifier" => return out(text(p, src).to_string()),
        "dotted_name" if kids.len() > 1 => return,
        "class_pattern" | "keyword_pattern" => 1,
        _ => 0,
    };
    for c in kids.into_iter().skip(skip) {
        captures(c, src, out);
    }
}

/// The identifiers a target binds: not an attribute's or a subscript's.
fn names(p: Node, src: &str, out: &mut impl FnMut(String)) {
    match p.kind() {
        "identifier" => out(text(p, src).to_string()),
        "attribute" | "subscript" => {}
        _ => {
            for c in p.named_children(&mut p.walk()) {
                names(c, src, out);
            }
        }
    }
}

/// A name or dotted name as the type, in path form (`a.b.C` as `a::b::C`);
/// `None` for any other annotation (`Optional[C]`, `C | None`, `"C"`).
fn type_name(t: Node, src: &str) -> Option<String> {
    let t = if t.kind() == "type" {
        t.named_child(0)?
    } else {
        t
    };
    matches!(t.kind(), "identifier" | "attribute")
        .then(|| one_line(text(t, src)).replace(' ', "").replace('.', "::"))
}

/// What one `import` or `from … import` statement imports.
fn statement_imports(n: Node, src: &str) -> Vec<Import> {
    let module = n
        .child_by_field_name("module_name")
        .map(|m| text(m, src).to_string());
    let mut out = Vec::new();
    for c in n.named_children(&mut n.walk()) {
        if Some(c) == n.child_by_field_name("module_name") {
            continue;
        }
        let (name, alias) = match c.kind() {
            "dotted_name" => (text(c, src).to_string(), None),
            "aliased_import" => (
                c.child_by_field_name("name")
                    .map_or("", |x| text(x, src))
                    .to_string(),
                c.child_by_field_name("alias")
                    .map(|x| text(x, src).to_string()),
            ),
            "wildcard_import" => ("*".to_string(), None),
            _ => continue,
        };
        out.push(match &module {
            Some(m) => Import {
                module: m.clone(),
                name: Some(name),
                alias,
            },
            None => Import {
                module: name,
                name: None,
                alias,
            },
        });
    }
    out
}

/// Does a statement at module level make a call or a decision? Imports,
/// `def`s, `class`es, docstrings and assignments of plain values do not.
pub(crate) fn module_code(a: &Analyzed) -> bool {
    a.sources.iter().any(|s| {
        let root = s.tree().root_node();
        let runs = root
            .named_children(&mut root.walk())
            .any(|n| match n.kind() {
                "import_statement"
                | "import_from_statement"
                | "future_import_statement"
                | "function_definition"
                | "class_definition"
                | "decorated_definition"
                | "type_alias_statement"
                | "pass_statement"
                | "comment" => false,
                "expression_statement" => {
                    let mut walk = n.walk();
                    let mut kids = n.named_children(&mut walk);
                    kids.any(|c| {
                        !matches!(c.kind(), "string" | "concatenated_string" | "assignment")
                    }) || decides(n)
                }
                _ => true,
            });
        runs
    })
}

/// A call, a conditional or a comprehension under `n`, outside any lambda.
fn decides(n: Node) -> bool {
    let mut stack = vec![n];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "call"
            | "conditional_expression"
            | "boolean_operator"
            | "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression"
            | "await" => return true,
            "lambda" => {}
            _ => stack.extend(n.named_children(&mut n.walk())),
        }
    }
    false
}

/// The imports of a Python file outside its functions, in source order.
pub(crate) fn module_imports(a: &Analyzed) -> Vec<Import> {
    let mut out = Vec::new();
    for s in &a.sources {
        let mut stack = vec![s.tree().root_node()];
        while let Some(n) = stack.pop() {
            match n.kind() {
                "function_definition" => {}
                "import_statement" | "import_from_statement" => {
                    out.extend(statement_imports(n, s.text()));
                }
                _ => {
                    let mut kids: Vec<Node> = n.named_children(&mut n.walk()).collect();
                    kids.reverse();
                    stack.extend(kids);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
