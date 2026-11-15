//! Python extraction: PEP 8 leading-underscore visibility convention,
//! `__all__` as the authoritative override, and AST-verified module docstring
//! extraction.
use super::code::{default_shape, LangSpec, Lead, Placement, Rank, Shape};
use std::collections::BTreeSet;
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator, Tree};

/// The `__all__` scan's query, compiled once per process rather than once per
/// Python file. See `code::QUERIES` for why.
static ALL_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language: Language = tree_sitter_python::LANGUAGE.into();
    Query::new(
        &language,
        r"
            (module
              (expression_statement
                [
                  (assignment left: (identifier) @lhs right: (_) @rhs)
                  (augmented_assignment left: (identifier) @lhs)
                  (call
                    function: (attribute
                      object: (identifier) @lhs
                      attribute: (identifier) @method))
                ]
              )
            )
        ",
    )
    .expect("__all__ query must compile")
});

/// Force this module's query to compile. See `code::validate_queries`.
pub(crate) fn validate_queries() {
    LazyLock::force(&ALL_QUERY);
}

/// A decorated definition parses as
/// `(decorated_definition (decorator)+ (function_definition|class_definition))`
/// and the `@def` capture lands on the inner node, so the decorator is
/// invisible without this. Decorators are kept **with their arguments**:
/// `@dataclass` makes a field a constructor parameter and `frozen=True` makes
/// it immutable, so cutting the arguments would hide the very semantics that
/// make the extracted fields interpretable.
///
/// One accepted risk, absent from the audit corpora: an unbounded argument
/// list (`@pytest.mark.parametrize("a,b", [(1, 2), …])`) is spliced in full.
///
/// A comment between the decorator and the definition (`@dataclass  # noqa`)
/// is not spliced in: `render_span` drops comment nodes.
///
/// An assignment climbs to its `expression_statement`, which starts at the
/// same byte: the statement, not the assignment, is the sibling of a `#`
/// comment above it.
fn python_sig_start(def: Node) -> Node {
    match def.parent() {
        Some(p) if matches!(p.kind(), "decorated_definition" | "expression_statement") => p,
        _ => def,
    }
}

fn keep_python_public(name: &str) -> bool {
    !name.starts_with('_')
}

/// Python's one grammar-specific shape. Everything else — `function_definition`,
/// `class_definition` — cuts at its `body` like any other language.
///
/// An assignment cuts at its value and re-appends it. Cutting rather than
/// keeping the source span is also what normalizes spacing: the ` = ` in a
/// rendered signature is always emitted by the join and never copied from
/// source, so `X="v"` and `X = "v"` render identically.
///
/// `has_type` is the one place a language answers it dynamically: only an
/// unannotated assignment lacks a type, and it is the one binding whose value
/// is truncated rather than omitted when over budget — a bare `SYSTEM_PROMPT`
/// with no kind, no type and no value would say nothing at all.
pub(crate) fn python_shape(def: Node) -> Shape {
    if def.kind() != "assignment" {
        return default_shape(def);
    }
    let right = def.child_by_field_name("right");
    Shape {
        rank: Rank::Value,
        cut: right,
        value: right,
        has_type: def.child_by_field_name("type").is_some(),
    }
}

// The grammar has no notion of "public"; capture every def/class
// and drop names starting with `_` in name_filter (PEP 8 convention).
//
// `from .models import X` and `from . import X` need separate
// patterns. In the second, `relative_import` is just an
// `import_prefix`, so its text is a lone `.` and
// `imports::Imports` — which resolves every non-empty segment
// of a split on `.` — finds no segment to resolve. The anchor `.`
// after `(import_prefix)` constrains it to be the last named
// child, which is exactly what separates the two families; without
// it, `from .models import Article, FeedSource` would also emit
// `Article` and `FeedSource` as imports.
//
// Known gap: the bare-prefix family loses its level, so
// `from . import x`, `from .. import x`, and `import x` all render
// as `x`. The graph is unaffected — the dropped level is not a
// resolvable segment. `from . import *` and
// `from __future__ import x`
// capture nothing; neither resolves to a page.
const QUERY: &str = r#"
                ; One pattern covers module constants AND class fields:
                ; tree-sitter matches at any depth, so a second class-scoped
                ; pattern would fire on the same nodes and emit every field twice.
                ; This is the trap the Rust cycle documented for `impl` methods.
                ;
                ; `left: (identifier)` means `HOST, PORT = "x", 80` (a
                ; `pattern_list`) and `X += 1` (an `augmented_assignment`) match
                ; nothing, and `A = B = 2` yields only `A` because the inner
                ; assignment is a descendant of `right:`, not of an
                ; `expression_statement`. All three are losses, not features.
                (expression_statement (assignment left: (identifier) @name) @def)
                (function_definition name: (identifier) @name) @def
                (class_definition name: (identifier) @name) @def
                (import_statement name: (dotted_name) @import)
                (import_statement name: (aliased_import name: (dotted_name) @import))
                (import_from_statement module_name: (dotted_name) @import)
                (import_from_statement module_name: (relative_import (dotted_name)) @import)
                (import_from_statement module_name: (relative_import (import_prefix) .) name: (dotted_name) @import)
                (import_from_statement module_name: (relative_import (import_prefix) .) name: (aliased_import name: (dotted_name) @import))
            "#;

pub(crate) fn python_spec() -> LangSpec {
    LangSpec {
        name_filter: keep_python_public,
        strip_trailing: &[':', '='],
        placement: python_placement,
        export_set: python_all,
        join_continuations: true,
        sig_start: python_sig_start,
        item_lead: python_lead,
        body_doc: python_body_doc,
        shape: python_shape,
        ..LangSpec::base("python", tree_sitter_python::LANGUAGE.into(), QUERY)
    }
}

/// A `#` (or Sphinx `#:`) comment above an item is a plain lead; a shebang or an encoding
/// declaration on the first two lines is not. Decorators sit inside the
/// `decorated_definition` that `sig_start` returns, so nothing else leads.
fn python_lead(node: Node, text: &str) -> Lead {
    match text
        .get(node.byte_range())
        .filter(|_| node.kind() == "comment")
        .and_then(|t| t.strip_prefix('#'))
    {
        Some(body)
            if !body.starts_with('!')
                && !(node.start_position().row <= 1
                    && (body.contains("coding:") || body.contains("coding="))) =>
        {
            Lead::Plain(comment_body(body).to_string())
        }
        _ => Lead::Stop,
    }
}

/// A `#` comment's text after its `#`, as a plain lead takes it: a Sphinx
/// attribute doc is `#:`.
pub(crate) fn comment_body(after_hash: &str) -> &str {
    after_hash.strip_prefix(':').unwrap_or(after_hash).trim()
}

/// Python's module-level guard and owner chain, resolved in one upward walk.
///
/// A **deny-list**, where Rust uses an allow-list, because the grammars are
/// mirror images: Rust has many kinds of item container, while Python's module
/// level has exactly one excluder — a function body. An allow-list would have
/// to enumerate `if_statement`, `try_statement`, `except_clause`,
/// `with_statement`, `match_statement` and more, and would silently drop the
/// `def`s under `if TYPE_CHECKING:` and `try: … except ImportError:` that are
/// genuine module-level exports.
///
/// The two conventions fail in opposite directions: a new node kind that can
/// host a definition is silently admitted here and would be silently rejected
/// by Rust's rule. A tree-sitter bump must be reviewed under both.
fn python_placement(def: Node, text: &str) -> Placement {
    let mut chain: Vec<String> = Vec::new();
    let mut node = def;
    while let Some(parent) = node.parent() {
        match parent.kind() {
            // `lambda` is defensive: no definition or assignment can be a
            // descendant of one in valid Python (a lambda body is an
            // expression, and a walrus is `named_expression`). Probed — it
            // never fires. Kept because absorbing a grammar change is cheaper
            // than noticing one.
            // A `def` directly in a function body; one in a class there is
            // a local class's method and stays out.
            "function_definition" if chain.is_empty() && super::code::is_function(def) => {
                return Placement::Nested;
            }
            "function_definition" | "lambda" => return Placement::Rejected,
            "class_definition" => {
                let Some(name) = parent
                    .child_by_field_name("name")
                    .and_then(|n| text.get(n.byte_range()))
                else {
                    return Placement::Rejected;
                };
                chain.push(name.to_string());
            }
            _ => {}
        }
        node = parent;
    }
    // The walk is bottom-up. Without this, `Article.Inner.deep` renders as
    // `Inner.Article.deep`.
    chain.reverse();
    Placement::Scoped(chain)
}

/// `__all__ = [...]` or `(...)` at module level, honored only when the LAST
/// module-level statement that touches `__all__` is a plain assignment to a
/// literal list/tuple of plain string literals. Anything else there —
/// `["a"] + other.__all__`, a comprehension, a bare name, a function call
/// (`__all__ = compute()`), an in-place mutation (`__all__ += [...]`,
/// `__all__.append(...)`, `__all__.extend(...)`), or an f-string element (its
/// node kind is also `string` in this grammar, but any `interpolation` child
/// means it is computed at import time exactly like the cases above) — means
/// the module computes or mutates its surface at import time, which no static
/// rule can follow, so the convention applies instead. An element with no
/// extractable `string_content` (a genuinely empty `""`) also aborts the set
/// and falls back to the convention; that is intended, not incidental, since
/// an empty name could never match a captured definition anyway.
///
/// "Last statement" is a property of the whole set of `__all__`-touching
/// statements, not just literal assignments: `__all__ = ["a"]` followed later
/// by `__all__ += ["b"]` must fall back to the convention (the augmented
/// assignment is last and isn't a literal assignment), even though the most
/// recent *literal* assignment looks well-formed on its own. Among literal
/// assignments alone, last wins: when a module reassigns `__all__` to a literal list
/// more than once with nothing else touching it in between, only the last one
/// governs.
///
/// Names listed but not defined in this file (the `__init__.py` re-export
/// case) match no captured definition and simply have no effect.
fn python_all(tree: &Tree, text: &str) -> Option<BTreeSet<String>> {
    let query = &*ALL_QUERY;
    let lhs_idx = query.capture_index_for_name("lhs");
    let rhs_idx = query.capture_index_for_name("rhs");
    let method_idx = query.capture_index_for_name("method");
    let mut cursor = QueryCursor::new();
    // Only the LAST `__all__`-touching statement matters. `last_literal_rhs`
    // is `Some` exactly when that statement is a plain assignment to a
    // `list`/`tuple` node; anything else touching `__all__` after a literal
    // assignment must clear it, which is why this is reset on every match
    // rather than only updated when a new literal is found.
    let mut last_literal_rhs: Option<Node> = None;
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        let mut lhs = None;
        let mut rhs = None;
        let mut method = None;
        for cap in m.captures() {
            if Some(cap.index) == lhs_idx {
                lhs = text.get(cap.node.byte_range());
            } else if Some(cap.index) == rhs_idx {
                rhs = Some(cap.node);
            } else if Some(cap.index) == method_idx {
                method = text.get(cap.node.byte_range());
            }
        }
        if lhs != Some("__all__") {
            continue;
        }
        // A call only counts as touching `__all__` when it is `.append` or
        // `.extend` — some other `__all__.something(...)` is out of scope.
        if let Some(name) = method {
            if name != "append" && name != "extend" {
                continue;
            }
        }
        last_literal_rhs = rhs.filter(|r| r.kind() == "list" || r.kind() == "tuple");
    }
    let rhs = last_literal_rhs?;
    let mut names = BTreeSet::new();
    let mut walker = rhs.walk();
    for el in rhs.named_children(&mut walker) {
        if el.kind() != "string" {
            return None;
        }
        // An f-string is also a `string` node; only an `interpolation` child
        // distinguishes it from a plain literal.
        let mut ic = el.walk();
        if el
            .named_children(&mut ic)
            .any(|n| n.kind() == "interpolation")
        {
            return None;
        }
        let mut sc = el.walk();
        let content = el
            .named_children(&mut sc)
            .find(|n| n.kind() == "string_content")?;
        names.insert(text.get(content.byte_range())?.to_string());
    }
    Some(names)
}

/// A def's or class's docstring: the first non-comment statement of its
/// `body`, when it is a string.
fn python_body_doc(def: Node, text: &str) -> Option<String> {
    if !matches!(def.kind(), "function_definition" | "class_definition") {
        return None;
    }
    docstring_paragraph(def.child_by_field_name("body")?, text)
}

/// The opening paragraph of `block`'s leading string statement, shared by a
/// module (its root) and a def (its `body`): the first non-comment statement,
/// if it is a bare string expression. Handles both single-line (`"""Doc."""`)
/// and multi-line (`"""\nDoc.\n"""`) docstrings, since it reads the AST's
/// `string_content` node rather than assuming the doc text is on line 1.
pub(crate) fn docstring_paragraph(block: Node, text: &str) -> Option<String> {
    let mut cursor = block.walk();
    let first_stmt = block
        .children(&mut cursor)
        .find(|n| n.kind() != "comment")?;
    if first_stmt.kind() != "expression_statement" {
        return None;
    }
    let mut inner_cursor = first_stmt.walk();
    let string_node = first_stmt
        .children(&mut inner_cursor)
        .find(|n| n.kind() == "string")?;
    let mut content_cursor = string_node.walk();
    let content_node = string_node
        .children(&mut content_cursor)
        .find(|n| n.kind() == "string_content")?;
    let raw = text.get(content_node.byte_range())?;
    // The opening paragraph, so a sentence wrapped across lines survives
    // `summarize`'s sentence cut.
    let paragraph: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty())
        .collect();
    (!paragraph.is_empty()).then(|| paragraph.join("\n"))
}

#[cfg(test)]
mod tests;
