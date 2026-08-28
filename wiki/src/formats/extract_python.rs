//! Python extraction: PEP 8 leading-underscore visibility convention,
//! `__all__` as the authoritative override, and AST-verified module docstring
//! extraction.
use super::code::{
    default_shape, keep_any_vis, no_header_group, LangSpec, Placement, Rank, Shape,
};
use std::collections::BTreeSet;
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Query, QueryCursor, Tree};

/// The `__all__` scan's query, compiled once per process rather than once per
/// Python file. See `code::QUERIES` for why.
static ALL_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language: Language = tree_sitter_python::LANGUAGE.into();
    Query::new(
        &language,
        r#"
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
        "#,
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
/// used to be spliced in with it. `render_span` drops comment nodes, so that
/// risk is closed.
pub(crate) fn python_sig_start(def: Node) -> Node {
    match def.parent() {
        Some(p) if p.kind() == "decorated_definition" => p,
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

pub(crate) fn python_spec() -> LangSpec {
    LangSpec {
        lang_name: "python",
        language: tree_sitter_python::LANGUAGE.into(),
        // The grammar has no notion of "public"; capture every def/class
        // and drop names starting with `_` in name_filter (PEP 8 convention).
        //
        // `from .models import X` and `from . import X` need separate
        // patterns. In the second, `relative_import` is just an
        // `import_prefix`, so its text is a lone `.` and
        // `graph::resolve_import` — which takes the last *non-empty*
        // segment of a split on `.` — extracts nothing. The anchor `.`
        // after `(import_prefix)` constrains it to be the last named
        // child, which is exactly what separates the two families; without
        // it, `from .models import Article, FeedSource` would also emit
        // `Article` and `FeedSource` as imports.
        //
        // Known gap: the bare-prefix family loses its level, so
        // `from . import x`, `from .. import x`, and `import x` all render
        // as `x`. The graph is unaffected — `resolve_import` discards the
        // prefix anyway. `from . import *` and `from __future__ import x`
        // capture nothing; neither resolves to a page.
        query_src: r#"
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
            "#,
        name_filter: keep_python_public,
        vis_filter: keep_any_vis,
        strip_trailing: &[':', '='],
        placement: python_placement,
        owner_sep: ".",
        export_set: python_all,
        join_continuations: true,
        sig_start: python_sig_start,
        header_group: no_header_group,
    }
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
pub(crate) fn python_placement(def: Node, text: &str) -> Placement {
    let mut chain: Vec<String> = Vec::new();
    let mut node = def;
    while let Some(parent) = node.parent() {
        match parent.kind() {
            // `lambda` is defensive: no definition or assignment can be a
            // descendant of one in valid Python (a lambda body is an
            // expression, and a walrus is `named_expression`). Probed — it
            // never fires. Kept because absorbing a grammar change is cheaper
            // than noticing one.
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
/// recent *literal* assignment looks well-formed on its own — reading only
/// literal assignments and ignoring everything between them is exactly the
/// defect this cycle exists to close. Among literal assignments alone,
/// last-wins is unchanged: when a module reassigns `__all__` to a literal list
/// more than once with nothing else touching it in between, only the last one
/// governs.
///
/// Names listed but not defined in this file (the `__init__.py` re-export
/// case) match no captured definition and simply have no effect.
pub(crate) fn python_all(tree: &Tree, text: &str) -> Option<BTreeSet<String>> {
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
    for m in cursor.matches(query, tree.root_node(), text.as_bytes()) {
        let mut lhs = None;
        let mut rhs = None;
        let mut method = None;
        for cap in m.captures {
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
            .find(|n| n.kind() == "string_content");
        match content.and_then(|n| text.get(n.byte_range())) {
            Some(s) => {
                names.insert(s.to_string());
            }
            None => return None,
        }
    }
    Some(names)
}

/// A Python module-level docstring: the first non-comment top-level
/// statement, if it is a bare string expression, with its first non-empty
/// inner line returned. Handles both single-line (`"""Doc."""`) and
/// multi-line (`"""\nDoc.\n"""`) docstrings, since it reads the AST's
/// `string_content` node rather than assuming the doc text is on line 1.
pub(crate) fn python_docstring(tree: &Tree, text: &str) -> Option<String> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let first_stmt = root.children(&mut cursor).find(|n| n.kind() != "comment")?;
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
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use crate::formats::code::CodeExtractor;
    use crate::formats::Extractor;
    use crate::formats::code::testutil::{assert_has, assert_lacks};

    #[test]
    fn python_signatures_gated_docstring_and_imports() {
        let src = "\"\"\"\nTop docstring.\n\"\"\"\nimport os\nfrom graph import build\ndef extract_all(d):\n    pass\ndef _private():\n    pass\n";
        let e = CodeExtractor.extract("extractor.py", src);
        assert_has(&e.symbols, "def extract_all(d)");
        assert!(!e.symbols.iter().any(|s| s.contains("_private")));
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert!(e.imports.iter().any(|i| i.contains("os")));
        assert_eq!(e.summary.as_deref(), Some("Top docstring."));
    }

    #[test]
    fn summary_fallback_prefers_a_definition_over_a_constant() {
        let src = "USER_AGENT = \"Mozilla/5.0\"\n\ndef fetch_feed(url: str) -> bytes:\n    return b\"\"\n";
        let e = CodeExtractor.extract("fetch.py", src);
        assert_eq!(
            e.summary.as_deref(),
            Some("def fetch_feed(url: str) -> bytes"),
            "an uppercase constant sorts first and must not win: {:?}",
            e.symbols
        );
    }

    #[test]
    fn summary_fallback_prefers_a_class_over_its_own_fields() {
        let src = "@dataclass(frozen=True)\nclass Article:\n    id: str\n    title: str\n";
        let e = CodeExtractor.extract("models.py", src);
        assert_eq!(
            e.summary.as_deref(),
            Some("@dataclass(frozen=True) class Article"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_relative_and_aliased_imports_are_captured() {
        let src = "import os\nimport aggregator.main as m\nfrom graph import build\nfrom .models import Article, FeedSource\nfrom . import state\nfrom . import helpers as h\nfrom ..pkg.sub import thing\n";
        let e = CodeExtractor.extract("main.py", src);
        assert_eq!(
            e.imports,
            vec![
                "..pkg.sub".to_string(),
                ".models".to_string(),
                "aggregator.main".to_string(),
                "graph".to_string(),
                "helpers".to_string(),
                "os".to_string(),
                "state".to_string(),
            ],
            "imports: {:?}",
            e.imports
        );
    }

    #[test]
    fn python_from_import_does_not_capture_symbol_names() {
        let src = "from .models import Article, FeedSource\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_eq!(
            e.imports,
            vec![".models".to_string()],
            "imports: {:?}",
            e.imports
        );
    }

    #[test]
    fn python_methods_are_qualified_by_their_class() {
        let src = "class Wiki:\n    def search(self, q: str) -> list:\n        return []\n    async def fetch(self) -> bytes:\n        return b\"\"\n\ndef free_function() -> int:\n    return 1\n";
        let e = CodeExtractor.extract("q.py", src);
        assert_has(&e.symbols, "def Wiki.search(self, q: str) -> list");
        assert_has(&e.symbols, "async def Wiki.fetch(self) -> bytes");
        assert_has(&e.symbols, "def free_function() -> int");
    }

    #[test]
    fn python_nested_class_chain_is_outermost_first() {
        let src =
            "class Article:\n    class Inner:\n        def deep(self) -> None:\n            pass\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "class Article.Inner");
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "def Article.Inner.deep(self) -> None"),
            "chain must be outermost-first, not Inner.Article: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_private_class_takes_its_methods_with_it() {
        // A public sibling class makes this test self-guarding: asserting only
        // `symbols.is_empty()` would equally pass if Python extraction were
        // disabled entirely. `Public` must survive so the empty methods of
        // `_TextExtractor` are known to have been gated, not simply never
        // extracted.
        let src = "class _TextExtractor:\n    def handle_data(self, data: str) -> None:\n        pass\n    def text(self) -> str:\n        return \"\"\n\nclass Public:\n    def run(self) -> None:\n        pass\n";
        let e = CodeExtractor.extract("parse.py", src);
        assert!(
            !e.symbols.iter().any(|s| s.contains("_TextExtractor")
                || s.contains("handle_data")
                || s.contains("text(")),
            "a private class must not export its methods: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "class Public"),
            "extraction must still be happening at all: {:?}",
            e.symbols
        );
        assert_has(&e.symbols, "def Public.run(self) -> None");
    }

    #[test]
    fn python_two_classes_with_the_same_method_stay_distinct() {
        let src = "class A2:\n    def run(self) -> None:\n        pass\n\nclass B2:\n    def run(self) -> None:\n        pass\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "def A2.run(self) -> None");
        assert_has(&e.symbols, "def B2.run(self) -> None");
    }

    #[test]
    fn exports_group_a_class_with_its_own_members() {
        let src =
            "@dataclass\nclass Article:\n    title: str\n\nclass Verdict:\n    decision: str\n";
        let e = CodeExtractor.extract("models.py", src);
        assert_eq!(
            e.symbols,
            vec![
                "@dataclass class Article".to_string(),
                "Article.title: str".to_string(),
                "class Verdict".to_string(),
                "Verdict.decision: str".to_string(),
            ],
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_function_local_definitions_are_not_module_exports() {
        let src = "def outer():\n    def nested():\n        pass\n    class LocalClass:\n        def m(self):\n            pass\n";
        let e = CodeExtractor.extract("t.py", src);
        assert_eq!(
            e.symbols,
            vec!["def outer()".to_string()],
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_all_is_the_authoritative_export_gate() {
        let src = "__all__ = [\"Public\", \"shown\"]\n\nclass Public:\n    field: str\n    def m(self) -> None:\n        pass\n    def _hidden(self) -> None:\n        pass\n\nclass NotListed:\n    field: int\n\ndef shown() -> int:\n    return 1\n\ndef not_listed() -> int:\n    return 2\n\nHIDDEN_CONST = 5\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "class Public");
        assert_has(&e.symbols, "def Public.m(self) -> None");
        assert_has(&e.symbols, "def shown() -> int");
        assert_lacks(&e.symbols, "NotListed");
        assert_lacks(&e.symbols, "not_listed");
        assert_lacks(&e.symbols, "HIDDEN_CONST");
        assert!(
            !e.symbols.iter().any(|s| s.contains("_hidden")),
            "__all__ says nothing about what is public inside a class: {:?}",
            e.symbols
        );
    }

    #[test]
    fn all_can_export_an_underscore_prefixed_free_name() {
        // `__all__` replaces the underscore convention for the *module-level*
        // name it lists — including a free item whose own name starts with
        // `_`. `__version__` is a real, common idiom; a re-application of
        // `name_filter` to the item's own name would silently override
        // `__all__` here instead of being replaced by it.
        let src = "__all__ = [\"_private_api\", \"__version__\", \"Public\"]\n\n__version__ = \"1.0\"\n\ndef _private_api() -> int:\n    return 1\n\nclass Public:\n    def _hidden(self) -> None:\n        pass\n";
        let e = CodeExtractor.extract("m.py", src);
        assert!(
            e.symbols.iter().any(|s| s == "def _private_api() -> int"),
            "__all__ must be able to export a free underscore-prefixed name: {:?}",
            e.symbols
        );
        assert_has(&e.symbols, "__version__ = \"1.0\"");
        assert_has(&e.symbols, "class Public");
        assert!(
            !e.symbols.iter().any(|s| s.contains("_hidden")),
            "a _hidden method of a listed class must stay hidden — the \
             convention always applies inside a class: {:?}",
            e.symbols
        );
    }

    #[test]
    fn without_all_the_underscore_convention_still_hides_a_free_name() {
        // Guards against an overcorrection of the fix above: with no `__all__`
        // at all, `exports` is `None` and the convention must still apply to
        // a free item's own name exactly as before.
        let src =
            "def _private_api() -> int:\n    return 1\n\ndef public_api() -> int:\n    return 2\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_lacks(&e.symbols, "_private_api");
        assert_has(&e.symbols, "def public_api() -> int");
    }

    #[test]
    fn a_computed_all_falls_back_to_the_underscore_convention() {
        let src = "__all__ = [\"a\"] + other.__all__\n\ndef a():\n    pass\n\ndef b():\n    pass\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "def a()");
        assert!(
            e.symbols.iter().any(|s| s == "def b()"),
            "a non-literal __all__ must not gate anything: {:?}",
            e.symbols
        );
    }

    #[test]
    fn an_all_containing_an_fstring_element_falls_back_to_the_underscore_convention() {
        let src =
            "x = 1\n__all__ = [f\"a{x}\", \"b\"]\n\ndef b():\n    pass\n\ndef c():\n    pass\n";
        let e = CodeExtractor.extract("m.py", src);
        assert!(
            e.symbols.iter().any(|s| s == "def c()"),
            "an f-string element must invalidate __all__, falling back to the convention: {:?}",
            e.symbols
        );
    }

    #[test]
    fn a_second_module_level_all_assignment_wins() {
        let src = "__all__ = [\"a\"]\ndef a():\n    pass\ndef b():\n    pass\n__all__ = [\"b\"]\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "def b()");
        assert!(
            !e.symbols.iter().any(|s| s == "def a()"),
            "the second, later __all__ must win over the first: {:?}",
            e.symbols
        );
    }

    #[test]
    fn an_augmented_all_after_a_literal_falls_back_to_the_underscore_convention() {
        // `__all__ += [...]` never matches the literal-assignment query shape
        // at all, so a prior literal assignment stayed visible and the
        // augmented one was invisible to `python_all` — the stale `["base_name"]`
        // kept governing and `added_name` was silently dropped even though it
        // is a real, file-defined, genuinely public name. The fix must see the
        // `+=` as the LAST statement touching `__all__` and fall back to the
        // convention for the whole module, exporting both names.
        let src = "__all__ = [\"base_name\"]\n__all__ += [\"added_name\"]\n\ndef base_name() -> int:\n    return 1\n\ndef added_name() -> int:\n    return 2\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "def base_name() -> int");
        assert!(
            e.symbols.iter().any(|s| s == "def added_name() -> int"),
            "`__all__ +=` must fall back to the convention, not keep the stale literal: {:?}",
            e.symbols
        );
    }

    #[test]
    fn a_recomputed_all_after_a_literal_falls_back_to_the_underscore_convention() {
        // `__all__ = compute()` after an earlier literal assignment: the
        // literal-only query saw only the first, literal assignment and never
        // noticed the module later recomputed its surface non-literally. The
        // recomputation must win as the last statement and fall back to the
        // convention.
        let src =
            "__all__ = [\"a\"]\ndef a():\n    pass\ndef b():\n    pass\n__all__ = compute()\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "def a()");
        assert!(
            e.symbols.iter().any(|s| s == "def b()"),
            "a non-literal reassignment must fall back to the convention: {:?}",
            e.symbols
        );
    }

    #[test]
    fn an_all_dot_extend_after_a_literal_falls_back_to_the_underscore_convention() {
        let src = "__all__ = [\"a\"]\n__all__.extend([\"b\"])\n\ndef a():\n    pass\n\ndef b():\n    pass\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "def a()");
        assert!(
            e.symbols.iter().any(|s| s == "def b()"),
            "__all__.extend(...) must fall back to the convention: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_assignments_keep_their_values_annotated_or_not() {
        let src = "MAX_IDS = 2000\nSUMMARY_LIMIT: int = 300\nPRISM_FILES: list[tuple[str, str]] = [(\"a\", \"b\")]\nAlias = list[int]\n_PRIVATE = 1\n";
        let e = CodeExtractor.extract("state.py", src);
        assert_has(&e.symbols, "MAX_IDS = 2000");
        assert_has(&e.symbols, "SUMMARY_LIMIT: int = 300");
        assert_has(&e.symbols, "PRISM_FILES: list[tuple[str, str]] = [(\"a\", \"b\")]");
        assert_has(&e.symbols, "Alias = list[int]");
        assert_lacks(&e.symbols, "_PRIVATE");
    }

    #[test]
    fn python_class_fields_are_qualified_and_gated() {
        let src = "class Article:\n    title: str\n    url: str = \"\"\n    published: datetime | None = None\n    _hidden: int = 0\n";
        let e = CodeExtractor.extract("models.py", src);
        assert_has(&e.symbols, "Article.title: str");
        assert_has(&e.symbols, "Article.url: str = \"\"");
        assert_has(&e.symbols, "Article.published: datetime | None = None");
        assert_lacks(&e.symbols, "_hidden");
    }

    #[test]
    fn python_class_fields_are_emitted_once_not_twice() {
        let src = "class Article:\n    title: str\n";
        let e = CodeExtractor.extract("m.py", src);
        let hits = e
            .symbols
            .iter()
            .filter(|s| *s == "Article.title: str")
            .count();
        assert_eq!(hits, 1, "one pattern, one emission: {:?}", e.symbols);
    }

    #[test]
    fn python_long_values_are_truncated_on_a_character_boundary() {
        // Cyrillic: a byte-indexed cut panics here. The real corpus has exactly
        // this shape in prism-agent's judge.py.
        let src = "SYSTEM_PROMPT = \"Ти — редакторський фільтр каналу, який оцінює статті дуже суворо\"\n";
        let e = CodeExtractor.extract("judge.py", src);
        let sig = e
            .symbols
            .iter()
            .find(|s| s.starts_with("SYSTEM_PROMPT"))
            .unwrap_or_else(|| panic!("no SYSTEM_PROMPT in {:?}", e.symbols));
        assert!(sig.ends_with('…'), "must be truncated: {sig}");
        assert!(sig.chars().count() < 80, "budget not applied: {sig}");
    }

    #[test]
    fn python_line_continuations_do_not_strand_the_strip_loop() {
        let src = "Z: int = \\\n    5\nX = \\\n    compute()\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "Z: int = 5");
        assert_has(&e.symbols, "X = compute()");
    }

    #[test]
    fn crlf_line_continuations_do_not_strand_the_strip_loop() {
        // Under CRLF the continuation sequence is `\` `\r` `\n`, not `\` `\n`
        // — a plain `.replace("\\\n", " ")` never matches it, so `Z: int = \`
        // survives unfixed on a CRLF file, exactly the defect the join was
        // added to prevent. The CRLF form must be replaced first.
        let src = "Z: int = \\\r\n    5\r\nX = \\\r\n    compute()\r\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "Z: int = 5");
        assert_has(&e.symbols, "X = compute()");
    }

    #[test]
    fn python_chained_assignment_yields_only_the_outer_name() {
        // Known loss, pinned so it is visible rather than surprising: `B` is
        // equally a public module-level name and is dropped — only `A` is
        // captured (the query anchors `assignment` as a direct child of
        // `expression_statement`; the nested `B = 2` sits under `right:`
        // instead). `A`'s value is kept whole, per the unannotated rule, and
        // that whole value happens to be the literal text `B = 2`.
        let src = "A = B = 2\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_eq!(e.symbols, vec!["A = B = 2".to_string()], "{:?}", e.symbols);
    }

    #[test]
    fn a_comment_between_a_decorator_and_its_definition_is_dropped() {
        // Previously documented on `python_sig_start` as an accepted risk:
        // the decorator span reaches the definition, so the comment came too.
        let src = "@dataclass  # noqa: D101\nclass Article:\n    title: str\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "@dataclass class Article");
        assert_lacks(&e.symbols, "noqa");
    }

    #[test]
    fn a_comment_inside_a_retained_value_is_dropped() {
        // The value is rendered through the same path as the head, so a
        // comment inside it was spliced in the same way.
        let src = "CONFIG = {  # inline\n    \"a\": 1,\n}\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "CONFIG = { \"a\": 1, }");
        assert_lacks(&e.symbols, "inline");
    }

    #[test]
    fn a_python_string_value_keeps_its_own_punctuation() {
        let src = "SEP = \" , \"\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "SEP = \" , \"");
    }

    #[test]
    fn python_decorators_are_kept_with_their_arguments() {
        let src = "@dataclass(frozen=True)\nclass Article:\n    title: str\n";
        let e = CodeExtractor.extract("models.py", src);
        assert_has(&e.symbols, "@dataclass(frozen=True) class Article");
        assert_has(&e.symbols, "Article.title: str");
    }

    #[test]
    fn python_method_decorators_render_before_the_qualified_name() {
        let src =
            "class Article:\n    @property\n    def slug(self) -> str:\n        return \"\"\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "@property def Article.slug(self) -> str");
    }

    #[test]
    fn python_stacked_decorators_all_survive() {
        let src = "@a\n@b(1)\n@c.d.e\ndef stacked() -> int:\n    return 0\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "@a @b(1) @c.d.e def stacked() -> int");
    }

    #[test]
    fn python_conditionally_defined_module_functions_are_kept() {
        let src = "if TYPE_CHECKING:\n    def type_only():\n        pass\n\ntry:\n    def optional_dep():\n        pass\nexcept ImportError:\n    pass\n";
        let e = CodeExtractor.extract("t.py", src);
        assert_has(&e.symbols, "def type_only()");
        assert_has(&e.symbols, "def optional_dep()");
    }

    #[test]
    fn python_unannotated_assignment_renders_its_value_once() {
        // `signature_cut` no longer stops short of an unannotated value, so the
        // head must not still carry it: `MAX_IDS = 2000 = 2000` is the failure.
        let src = "MAX_IDS = 2000\n";
        let e = CodeExtractor.extract("state.py", src);
        assert_has(&e.symbols, "MAX_IDS = 2000");
        assert!(
            !e.symbols.iter().any(|s| s.matches(" = ").count() > 1),
            "value appended twice: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_assignment_without_spaces_is_normalized_and_budgeted() {
        // The ` = ` is emitted by the join now, never copied from source, so a
        // no-space assignment renders like every sibling. Previously
        // `truncate_value`'s `sig.find(" = ")` missed this line entirely and
        // the budget was bypassed however long the value was.
        let long = "y".repeat(200);
        let src = format!("CACHE_DIR=\"/var/cache/store\"\nBIG=\"{long}\"\n");
        let e = CodeExtractor.extract("state.py", &src);
        assert_has(&e.symbols, "CACHE_DIR = \"/var/cache/store\"");
        let big = e
            .symbols
            .iter()
            .find(|s| s.starts_with("BIG"))
            .unwrap_or_else(|| panic!("no BIG in {:?}", e.symbols));
        assert!(big.starts_with("BIG = "), "normalized spacing: {big}");
        assert!(big.ends_with('…'), "budget must apply: {big}");
    }

    #[test]
    fn python_annotated_over_budget_omits_the_value() {
        // An annotation survives to carry the contract, so omit rather than
        // truncate — the same rule Rust follows, for the same reason.
        let long = "z".repeat(200);
        let src = format!("BIG: str = \"{long}\"\n");
        let e = CodeExtractor.extract("state.py", &src);
        assert_has(&e.symbols, "BIG: str");
        assert!(
            !e.symbols.iter().any(|s| s.contains('…')),
            "annotated must omit, not truncate: {:?}",
            e.symbols
        );
    }

    #[test]
    fn a_continuation_inside_the_value_is_joined() {
        // The continuation is a child of the `binary_operator` that IS the
        // `right` field — it is inside the value, not between `=` and the
        // value. Without the shared pipeline this renders `X = 1 + \ 2`.
        let src = "X = 1 + \\\n    2\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "X = 1 + 2");
    }

    #[test]
    fn a_crlf_continuation_inside_the_value_is_joined() {
        let src = "X = 1 + \\\r\n    2\r\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "X = 1 + 2");
    }

    #[test]
    fn a_wrapped_call_value_keeps_its_punctuation_tidied() {
        // Without the shared pipeline this renders `Y = compute( 1, 2, )`.
        let src = "Y = compute(\n    1,\n    2,\n)\n";
        let e = CodeExtractor.extract("m.py", src);
        assert_has(&e.symbols, "Y = compute(1, 2)");
    }

    #[test]
    fn a_const_only_module_does_not_summarize_as_a_truncated_value() {
        // `pick_summary_fallback` reaches FreeValue when a module has no
        // FreeDef and no docstring, and summaries are scored at W_SUMMARY=1.5
        // in search — higher than body text. A truncated fragment must never
        // land there.
        let long = "q".repeat(200);
        let src = format!("TOPICS: list[str] = [\"{long}\"]\n");
        let e = CodeExtractor.extract("settings.py", &src);
        let summary = e.summary.unwrap_or_default();
        assert!(
            !summary.contains('…'),
            "summary must not be a truncated value: {summary}"
        );
    }
}
