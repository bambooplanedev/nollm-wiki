//! JS, TS, and Go: languages whose export surface is expressed by the grammar
//! (`export_statement`) or by a naming convention (Go's leading capital), with
//! no owner resolution and no scope guard.
use super::code::{
    default_shape, is_function, outermost, plain_comment_text, value_shape, LangSpec, Lead,
    Placement, Rank, Shape,
};
use tree_sitter::{Language, Node};

fn keep_go_exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

/// The JS and TS specs differ only in grammar and in the node kind a class
/// name carries; everything else is shared here.
fn ecma_spec(lang_name: &'static str, language: Language, query_src: &'static str) -> LangSpec {
    LangSpec {
        // `const twice = …` is cut at its value, which leaves the `=`.
        strip_trailing: &['='],
        placement: ecma_placement,
        sig_start: ecma_sig_start,
        item_lead: ecma_lead,
        // The query has no per-member visibility, so a method of an exported
        // class reads private: a known gap. Only an `export` @def is public.
        query_gates_visibility: true,
        shape: ecma_shape,
        ..LangSpec::base(lang_name, language, query_src)
    }
}

pub(crate) fn js_spec() -> LangSpec {
    ecma_spec(
        "javascript",
        tree_sitter_javascript::LANGUAGE.into(),
        // Only symbols wrapped in an `export_statement` are captured at
        // all, so a bare `function helper() {}` never matches.
        r"
                (export_statement declaration: (function_declaration name: (identifier) @name)) @def
                (export_statement declaration: (class_declaration name: (identifier) @name)) @def
                (function_declaration name: (identifier) @name) @item
                (generator_function_declaration name: (identifier) @name) @item
                (method_definition name: [(property_identifier) (private_property_identifier)] @name) @item
                (variable_declarator name: (identifier) @name) @item
                (class_declaration name: (identifier) @name) @item
                (import_statement source: (string) @import)
            ",
    )
}

pub(crate) fn ts_spec() -> LangSpec {
    ecma_spec(
        "typescript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        r"
                (export_statement declaration: (function_declaration name: (identifier) @name)) @def
                (export_statement declaration: (class_declaration name: (type_identifier) @name)) @def
                (function_declaration name: (identifier) @name) @item
                (generator_function_declaration name: (identifier) @name) @item
                (method_definition name: [(property_identifier) (private_property_identifier)] @name) @item
                (variable_declarator name: (identifier) @name) @item
                (class_declaration name: (type_identifier) @name) @item
                (abstract_class_declaration name: (type_identifier) @name) @item
                (interface_declaration name: (type_identifier) @name) @item
                (type_alias_declaration name: (type_identifier) @name) @item
                (enum_declaration name: (identifier) @name) @item
                (enum_body (property_identifier) @name @item)
                (enum_body (enum_assignment name: (property_identifier) @name) @item)
                (import_statement source: (string) @import)
            ",
    )
}

/// Functions and methods hold no items of their own (a nested helper is not
/// part of the file's surface), while a class names its methods' owner.
fn ecma_placement(def: Node, text: &str) -> Placement {
    let mut chain = Vec::new();
    let mut up = def.parent();
    while let Some(n) = up {
        match n.kind() {
            "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "function"
            | "arrow_function"
            | "method_definition"
            | "generator_function" => return Placement::Rejected,
            "class_declaration" | "class" | "abstract_class_declaration" | "enum_declaration" => {
                if let Some(name) = n
                    .child_by_field_name("name")
                    .and_then(|n| text.get(n.byte_range()))
                {
                    chain.push(name.to_string());
                }
            }
            _ => {}
        }
        up = n.parent();
    }
    chain.reverse();
    Placement::Scoped(chain)
}

/// A variable whose value is not a function is a value. It keeps the shared
/// cut at its value, so `const LIMIT = 5` reads `const LIMIT`.
pub(crate) fn ecma_shape(def: Node) -> Shape {
    // An enum member reads with its value: `Dark = 3`.
    if def.kind() == "enum_assignment" {
        let eq = def.children(&mut def.walk()).find(|c| c.kind() == "=");
        return value_shape(eq, def.child_by_field_name("value"), false);
    }
    let rank = if def.kind() == "variable_declarator" && !is_function(def) {
        Rank::Value
    } else {
        Rank::Def
    };
    Shape {
        rank,
        ..default_shape(def)
    }
}

/// A declaration's range and signature start at its `export` or `const`.
fn ecma_sig_start(def: Node) -> Node {
    outermost(def, |k| {
        matches!(
            k,
            "lexical_declaration" | "variable_declaration" | "export_statement"
        )
    })
}

/// A `JSDoc` block (`/** … */`) above a declaration is its doc, and any other
/// comment a plain lead.
fn ecma_lead(node: Node, text: &str) -> Lead {
    match text
        .get(node.byte_range())
        .filter(|_| node.kind() == "comment")
    {
        Some(t) if t.starts_with("/**") && !t.starts_with("/***") && !t.starts_with("/**/") => {
            let body = t.trim_start_matches("/**").trim_end_matches("*/");
            let lines: Vec<&str> = body
                .lines()
                .map(|l| l.trim().trim_start_matches('*').trim())
                .filter(|l| !l.is_empty())
                .collect();
            Lead::Doc(lines.join("\n"))
        }
        // A triple-slash directive (`/// <reference …/>`) is no doc.
        Some(t) if t.starts_with("/// <") => Lead::Stop,
        Some(t) => Lead::Plain(plain_comment_text(t)),
        None => Lead::Stop,
    }
}

// Go has no `export` keyword; "exported" means the identifier's
// first rune is uppercase. The query can't express that, so it
// captures every func/type and name_filter drops the unexported ones.
// A spec's names are matched without their `name` field: the grammar
// puts the field on the commas of `const B, C = 2, 3` too, so the
// field form never reaches `C`. A spec's only direct `identifier`
// children are its names (a type is a `type_identifier`).
const QUERY: &str = r"
                (function_declaration name: (identifier) @name) @def
                (type_declaration (type_spec name: (type_identifier) @name)) @def
                (method_declaration name: (field_identifier) @name) @item
                (const_spec (identifier) @name) @item
                (var_spec (identifier) @name) @item
                (import_spec path: (interpreted_string_literal) @import)
            ";

pub(crate) fn go_spec() -> LangSpec {
    LangSpec {
        name_filter: keep_go_exported,
        placement: go_placement,
        owner_sep: "",
        sig_start: go_sig_start,
        item_lead: go_lead,
        shape: go_shape,
        ..LangSpec::base("go", tree_sitter_go::LANGUAGE.into(), QUERY)
    }
}

/// A method belongs to its receiver's type: `*Server`, `Server` and
/// `*Server[T]` all give `Server`. Everything else is a free item, except a
/// `const` or `var` inside a function, which is no item at all.
fn go_placement(def: Node, text: &str) -> Placement {
    let local = matches!(def.kind(), "const_spec" | "var_spec")
        && std::iter::successors(def.parent(), Node::parent).any(|n| {
            matches!(
                n.kind(),
                "function_declaration" | "method_declaration" | "func_literal"
            )
        });
    if local {
        return Placement::Rejected;
    }
    let receiver = (def.kind() == "method_declaration")
        .then(|| def.child_by_field_name("receiver"))
        .flatten()
        .and_then(|r| r.named_child(0))
        .and_then(|p| p.child_by_field_name("type"))
        .and_then(|t| text.get(t.byte_range()));
    Placement::Scoped(
        receiver
            .map(|t| {
                let t = t.trim_start_matches('*');
                t.split('[').next().unwrap_or(t).trim().to_string()
            })
            .into_iter()
            .collect(),
    )
}

/// An ungrouped `const X = 1` starts at its `const`, where its doc comment
/// sits; a spec inside `const ( … )` or `var ( … )` starts at itself, so
/// each spec keeps its own lines and doc.
fn go_sig_start(def: Node) -> Node {
    match def.parent() {
        Some(p)
            if matches!(p.kind(), "const_declaration" | "var_declaration")
                && !p.children(&mut p.walk()).any(|c| c.kind() == "(") =>
        {
            p
        }
        _ => def,
    }
}

/// A `const`/`var` spec is a value: it stops at its `=` and re-appends the
/// value within `VALUE_BUDGET`. Everything else is the shared default.
pub(crate) fn go_shape(def: Node) -> Shape {
    if !matches!(def.kind(), "const_spec" | "var_spec") {
        return default_shape(def);
    }
    let eq = def.children(&mut def.walk()).find(|c| c.kind() == "=");
    let value = def.child_by_field_name("value");
    value_shape(eq, value, def.child_by_field_name("type").is_some())
}

/// Go's doc convention is the `//` comment lines directly above a declaration.
/// A directive (`//go:generate`, `//nolint`, `//export F`) sits in the same
/// place but instructs a tool, so it belongs to the range and not the doc.
fn go_lead(node: Node, text: &str) -> Lead {
    match text
        .get(node.byte_range())
        .and_then(|t| t.strip_prefix("//"))
    {
        Some(doc) if node.kind() == "comment" && is_go_directive(doc) => Lead::Attr,
        Some(doc) if node.kind() == "comment" => Lead::Doc(doc.trim().to_string()),
        _ => Lead::Stop,
    }
}

/// A `//` comment body that is a directive: the named forms, or the general
/// `//tool:args` shape, which has no space after `//`.
fn is_go_directive(body: &str) -> bool {
    let general = body.split_once(':').is_some_and(|(tool, rest)| {
        let lower_alnum = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
        !tool.is_empty() && tool.chars().all(lower_alnum) && rest.starts_with(lower_alnum)
    });
    general
        || ["go:", "line ", "export ", "extern ", "nolint"]
            .iter()
            .any(|p| body.starts_with(p))
}

#[cfg(test)]
mod tests;
