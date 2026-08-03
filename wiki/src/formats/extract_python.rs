//! Python extraction: PEP 8 leading-underscore visibility convention and
//! AST-verified module docstring extraction.
use super::code::{always_free, keep_any_vis, LangSpec};
use tree_sitter::Tree;

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
        placement: always_free,
        owner_sep: "",
    }
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
}
