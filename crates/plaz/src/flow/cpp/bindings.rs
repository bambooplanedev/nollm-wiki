//! The names a C/C++ function or lambda binds.

use crate::flow::{text, Bindings};
use crate::source::pointee;
use std::collections::BTreeSet;
use wiki_code::support::one_line;
use wiki_code::tree_sitter::Node;

/// Every name the function or lambda `root` binds: parameters, captures,
/// locals (in conditions, `for` initializers and range `for`s too),
/// handler parameters and structured bindings. A name is typed by its
/// written type, `const`, `volatile`, `&` and `*` left out, a primitive
/// type being none; `auto` only by `T{…}` and `new T` (as `T`) and by
/// `make_unique<T>`/`make_shared<T>` (as the smart pointer). Lambdas and
/// functions under `root` bind for their own flows. `rebound` holds the
/// template parameters around `root` and the types the body declares; a
/// binding of one of them has no type.
pub(super) fn bindings(root: Node, src: &str) -> Bindings {
    let mut binds = Vec::new();
    let mut rebound = template_params(root, src);
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        let field = |f: &str| n.child_by_field_name(f);
        match n.kind() {
            "lambda_expression" | "function_definition" if n != root => continue,
            "function_definition" | "lambda_expression" | "catch_clause" => {
                let list = match n.kind() {
                    "catch_clause" => field("parameters"),
                    _ => field("declarator").and_then(parameters),
                };
                for l in list.into_iter().chain(field("captures")) {
                    params(l, src, &mut binds);
                }
            }
            "declaration" | "for_range_loop" => {
                let ty = field("type");
                for d in n.children_by_field_name("declarator", &mut n.walk()) {
                    // `T f();` declares a function, not a local.
                    if d.kind() == "function_declarator" {
                        continue;
                    }
                    let value = match d.kind() {
                        "init_declarator" => d.child_by_field_name("value"),
                        _ => field("value"),
                    };
                    let ty = ty.and_then(|t| match t.kind() {
                        "placeholder_type_specifier" => value.and_then(|v| made(v, src)),
                        _ => written(t, src),
                    });
                    bind(d, ty, src, &mut binds);
                }
            }
            "alias_declaration" => rebound.extend(field("name").map(|i| text(i, src).to_string())),
            "class_specifier" | "struct_specifier" | "union_specifier" | "enum_specifier"
                if field("body").is_some() =>
            {
                rebound.extend(field("name").map(|i| text(i, src).to_string()));
            }
            "type_definition" => {
                for d in n.children_by_field_name("declarator", &mut n.walk()) {
                    rebound.extend(declared(d).map(|(i, _)| text(i, src).to_string()));
                }
            }
            _ => {}
        }
        stack.extend(n.named_children(&mut n.walk()));
    }
    let keep = |t: &String| !rebound.contains(class_root(t));
    // A missing name of a damaged declaration is empty.
    let binds = (binds.into_iter())
        .filter(|(n, _)| !n.is_empty())
        .map(|(n, t)| (n, t.filter(keep)));
    let joined = Bindings::join(binds);
    // A method in a class body inside a function body (a macro block such
    // as `EACH(x) { … }` is a function definition in no class body).
    let mut around = std::iter::successors(root.parent(), Node::parent);
    let in_class = root
        .parent()
        .is_some_and(|p| p.kind() == "field_declaration_list");
    Bindings {
        rebound,
        local_class: in_class && around.any(|n| n.kind() == "compound_statement"),
        ..joined
    }
}

/// The first segment of the class a call on the written type `t` lands
/// in (`pointee`): `T` of `std::unique_ptr<T const>`, `a` of `a::B<C>`.
/// A template parameter or local type there makes `t` no project class;
/// other template arguments do not change the class.
fn class_root(t: &str) -> &str {
    let class = pointee(t).1;
    class.split(['<', ':']).next().unwrap_or("").trim()
}

/// The data members the class body `body` declares, those in `#if`
/// groups included, each typed as a local is; a function pointer member,
/// or one whose type names a template parameter in `params`, untyped.
pub(crate) fn fields(
    body: Node,
    src: &str,
    params: &BTreeSet<&str>,
) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut stack: Vec<Node> = body.named_children(&mut body.walk()).collect();
    while let Some(n) = stack.pop() {
        if n.kind().starts_with("preproc_") {
            stack.extend(n.named_children(&mut n.walk()));
        }
        if n.kind() != "field_declaration" {
            continue;
        }
        let ty = n.child_by_field_name("type").and_then(|t| written(t, src));
        let ty = ty.filter(|t| !params.contains(class_root(t)));
        for d in n.children_by_field_name("declarator", &mut n.walk()) {
            // `int* f();` declares a method; `int (*f)();` a member.
            let method = declared(d).is_some_and(|(i, function)| {
                function
                    && i.parent()
                        .is_some_and(|p| p.kind() == "function_declarator")
            });
            if !method {
                bind(d, ty.clone(), src, &mut out);
            }
        }
    }
    out.retain(|(n, _)| !n.is_empty());
    out
}

/// The names of the template parameters of each template around `root`,
/// and of `root` and each lambda around it.
fn template_params(root: Node, src: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut at = Some(root);
    while let Some(t) = at {
        let ps = match t.kind() {
            "template_declaration" => t.child_by_field_name("parameters"),
            "lambda_expression" => t.child_by_field_name("template_parameters"),
            _ => None,
        };
        if let Some(ps) = ps {
            for p in ps.named_children(&mut ps.walk()) {
                out.extend(template_param(p).map(|n| text(n, src).to_string()));
            }
        }
        at = t.parent();
    }
    out
}

/// The names a parameter list or a lambda's captures bind into `out`; a
/// K&R parameter and a capture are bare names, untyped.
fn params(list: Node, src: &str, out: &mut Vec<(String, Option<String>)>) {
    for p in list.named_children(&mut list.walk()) {
        match p.kind() {
            "identifier" => out.push((text(p, src).to_string(), None)),
            "lambda_capture_initializer" => out
                .extend((p.child_by_field_name("left")).map(|i| (text(i, src).to_string(), None))),
            _ => {
                let ty = p.child_by_field_name("type").and_then(|t| written(t, src));
                if let Some(d) = p.child_by_field_name("declarator") {
                    bind(d, ty, src, out);
                }
            }
        }
    }
}

/// The name declarator `d` binds into `out`, typed `ty` unless a function
/// declarator wraps it; each name of a structured binding untyped.
fn bind(d: Node, ty: Option<String>, src: &str, out: &mut Vec<(String, Option<String>)>) {
    let Some((d, function)) = declared(d) else {
        return;
    };
    if d.kind() == "structured_binding_declarator" {
        for i in d.named_children(&mut d.walk()) {
            if i.kind() == "identifier" {
                out.push((text(i, src).to_string(), None));
            }
        }
    } else {
        out.push((text(d, src).to_string(), ty.filter(|_| !function)));
    }
}

/// The name a declarator declares under its pointer, reference, array,
/// pack, initializer and parentheses wrappers, and whether a function
/// declarator wraps it (a function pointer); a structured binding whole;
/// `None` for an abstract or a damaged declarator.
fn declared(mut d: Node) -> Option<(Node, bool)> {
    let mut function = false;
    loop {
        match d.kind() {
            "identifier"
            | "field_identifier"
            | "type_identifier"
            | "structured_binding_declarator" => return Some((d, function)),
            "function_declarator" => function = true,
            "pointer_declarator"
            | "reference_declarator"
            | "array_declarator"
            | "init_declarator"
            | "parenthesized_declarator"
            | "variadic_declarator"
            | "attributed_declarator" => {}
            _ => return None,
        }
        d = d
            .child_by_field_name("declarator")
            .or_else(|| d.named_children(&mut d.walk()).last())?;
    }
}

/// The parameter list of the innermost function declarator in `d`: a
/// function's own, not that of a function pointer it returns.
fn parameters(d: Node) -> Option<Node> {
    let mut list = None;
    let mut at = Some(d);
    while let Some(n) = at {
        if n.kind().ends_with("function_declarator") {
            list = n.child_by_field_name("parameters").or(list);
        }
        at = n.child_by_field_name("declarator").or_else(|| {
            (n.kind() == "parenthesized_declarator" || n.kind() == "reference_declarator")
                .then(|| n.named_child(0))
                .flatten()
        });
    }
    list
}

/// The name a template parameter declares, if it has one.
fn template_param(p: Node) -> Option<Node> {
    let last = || p.named_children(&mut p.walk()).last();
    match p.kind() {
        "type_parameter_declaration" | "variadic_type_parameter_declaration" => {
            last().filter(|n| n.kind() == "type_identifier")
        }
        "template_template_parameter_declaration" => last().and_then(template_param),
        _ => p
            .child_by_field_name("name")
            .or_else(|| Some(declared(p.child_by_field_name("declarator")?)?.0)),
    }
}

/// A written type as a binding's type: a name, path or template type as
/// written, or the name of `struct X`; none for a primitive or any other.
fn written(t: Node, src: &str) -> Option<String> {
    match t.kind() {
        "type_identifier" | "qualified_identifier" | "template_type" => {
            Some(one_line(text(t, src)))
        }
        "struct_specifier" | "class_specifier" | "union_specifier" | "enum_specifier" => t
            .child_by_field_name("name")
            .map(|n| one_line(text(n, src))),
        _ => None,
    }
}

/// The type `auto` takes from the value `v`: `T` of `T{…}` and `new T`,
/// `std::unique_ptr<T>` of `make_unique<T>(…)`, `std::shared_ptr<T>` of
/// `make_shared<T>(…)`; none from any other call.
fn made(v: Node, src: &str) -> Option<String> {
    match v.kind() {
        "compound_literal_expression" | "new_expression" => {
            written(v.child_by_field_name("type")?, src)
        }
        "call_expression" => {
            let f = v.child_by_field_name("function")?;
            let t = match f.kind() {
                "qualified_identifier" => f.child_by_field_name("name")?,
                _ => f,
            };
            let args = t
                .child_by_field_name("arguments")
                .filter(|_| t.kind() == "template_function")?;
            let ptr = match &src[f.start_byte()..args.start_byte()] {
                "make_unique" | "std::make_unique" => "unique_ptr",
                "make_shared" | "std::make_shared" => "shared_ptr",
                _ => return None,
            };
            Some(format!("std::{ptr}{}", one_line(text(args, src))))
        }
        _ => None,
    }
}
