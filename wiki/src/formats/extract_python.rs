//! Python extraction: PEP 8 leading-underscore visibility convention,
//! `__all__` as the authoritative override, and AST-verified module docstring
//! extraction.
use super::code::{keep_any_vis, LangSpec, Placement};
use std::collections::BTreeSet;
use tree_sitter::{Language, Node, Query, QueryCursor, Tree};

fn keep_python_public(name: &str) -> bool {
    !name.starts_with('_')
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
        strip_trailing: &[':'],
        placement: python_placement,
        owner_sep: ".",
        export_set: python_all,
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

/// `__all__ = [...]` or `(...)` at module level, honored only when every
/// element is a plain string literal. Anything else — `["a"] + other.__all__`,
/// a comprehension, a bare name, an f-string (its node kind is also `string`
/// in this grammar, but any `interpolation` child means it is computed at
/// import time exactly like the cases above) — means the module computes its
/// surface at import time, which no static rule can follow, so the
/// convention applies instead. An element with no extractable
/// `string_content` (a genuinely empty `""`) also aborts the set and falls
/// back to the convention; that is intended, not incidental, since an empty
/// name could never match a captured definition anyway.
///
/// Python assignment is last-wins, so when a module reassigns `__all__` more
/// than once at module level, only the last assignment governs; earlier ones
/// are ignored entirely, including their validity.
///
/// Names listed but not defined in this file (the `__init__.py` re-export
/// case) match no captured definition and simply have no effect.
pub(crate) fn python_all(tree: &Tree, text: &str) -> Option<BTreeSet<String>> {
    let language: Language = tree_sitter_python::LANGUAGE.into();
    let query = Query::new(
        &language,
        "(module (expression_statement (assignment left: (identifier) @lhs right: [(list) (tuple)] @rhs)))",
    )
    .ok()?;
    let lhs_idx = query.capture_index_for_name("lhs");
    let rhs_idx = query.capture_index_for_name("rhs");
    let mut cursor = QueryCursor::new();
    let mut last_rhs: Option<Node> = None;
    for m in cursor.matches(&query, tree.root_node(), text.as_bytes()) {
        let mut lhs = None;
        let mut rhs = None;
        for cap in m.captures {
            if Some(cap.index) == lhs_idx {
                lhs = text.get(cap.node.byte_range());
            } else if Some(cap.index) == rhs_idx {
                rhs = Some(cap.node);
            }
        }
        if lhs == Some("__all__") {
            last_rhs = rhs;
        }
    }
    let rhs = last_rhs?;
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

    #[test]
    fn python_signatures_gated_docstring_and_imports() {
        let src = "\"\"\"\nTop docstring.\n\"\"\"\nimport os\nfrom graph import build\ndef extract_all(d):\n    pass\ndef _private():\n    pass\n";
        let e = CodeExtractor.extract("extractor.py", src);
        assert!(
            e.symbols.iter().any(|s| s == "def extract_all(d)"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("_private")));
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert!(e.imports.iter().any(|i| i.contains("os")));
        assert_eq!(e.summary.as_deref(), Some("Top docstring."));
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
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "def Wiki.search(self, q: str) -> list"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "async def Wiki.fetch(self) -> bytes"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "def free_function() -> int"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_nested_class_chain_is_outermost_first() {
        let src =
            "class Article:\n    class Inner:\n        def deep(self) -> None:\n            pass\n";
        let e = CodeExtractor.extract("m.py", src);
        assert!(
            e.symbols.iter().any(|s| s == "class Article.Inner"),
            "symbols: {:?}",
            e.symbols
        );
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
        let src = "class _TextExtractor:\n    def handle_data(self, data: str) -> None:\n        pass\n    def text(self) -> str:\n        return \"\"\n";
        let e = CodeExtractor.extract("parse.py", src);
        assert!(
            e.symbols.is_empty(),
            "a private class must not export its methods: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_two_classes_with_the_same_method_stay_distinct() {
        let src = "class A2:\n    def run(self) -> None:\n        pass\n\nclass B2:\n    def run(self) -> None:\n        pass\n";
        let e = CodeExtractor.extract("m.py", src);
        assert!(
            e.symbols.iter().any(|s| s == "def A2.run(self) -> None"),
            "{:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "def B2.run(self) -> None"),
            "{:?}",
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
        assert!(
            e.symbols.iter().any(|s| s == "class Public"),
            "{:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "def Public.m(self) -> None"),
            "{:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "def shown() -> int"),
            "{:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("NotListed")),
            "{:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("not_listed")),
            "{:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("HIDDEN_CONST")),
            "{:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("_hidden")),
            "__all__ says nothing about what is public inside a class: {:?}",
            e.symbols
        );
    }

    #[test]
    fn a_computed_all_falls_back_to_the_underscore_convention() {
        let src = "__all__ = [\"a\"] + other.__all__\n\ndef a():\n    pass\n\ndef b():\n    pass\n";
        let e = CodeExtractor.extract("m.py", src);
        assert!(e.symbols.iter().any(|s| s == "def a()"), "{:?}", e.symbols);
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
        assert!(e.symbols.iter().any(|s| s == "def b()"), "{:?}", e.symbols);
        assert!(
            !e.symbols.iter().any(|s| s == "def a()"),
            "the second, later __all__ must win over the first: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_conditionally_defined_module_functions_are_kept() {
        let src = "if TYPE_CHECKING:\n    def type_only():\n        pass\n\ntry:\n    def optional_dep():\n        pass\nexcept ImportError:\n    pass\n";
        let e = CodeExtractor.extract("t.py", src);
        assert!(
            e.symbols.iter().any(|s| s == "def type_only()"),
            "{:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "def optional_dep()"),
            "{:?}",
            e.symbols
        );
    }
}
