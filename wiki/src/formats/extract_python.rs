//! Python extraction: PEP 8 leading-underscore visibility convention and
//! AST-verified module docstring extraction.
use super::code::{any_def, keep_any_vis, no_owner, LangSpec};
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
        query_src: r#"
                (function_definition name: (identifier) @name) @def
                (class_definition name: (identifier) @name) @def
                (import_statement name: (dotted_name) @import)
                (import_from_statement module_name: (dotted_name) @import)
            "#,
        name_filter: keep_python_public,
        vis_filter: keep_any_vis,
        strip_trailing: &[':'],
        owner_of: no_owner,
        owner_sep: "",
        def_filter: any_def,
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
}
