//! JSX and TSX: the JS and TS specs from `extract_simple`, reused unchanged
//! except for the grammar that has to accept JSX in expression position.
//!
//! The export surface of a component file is the same `export_statement` the
//! plain-language specs gate on — a JSX element is a value inside a body, and
//! the signature cut already stops at the body, so no element ever reaches a
//! signature.
use super::code::LangSpec;
use super::extract_simple::{js_spec, ts_spec};

/// `tree_sitter_javascript` parses JSX natively, so the JS spec already is the
/// JSX spec. A function of its own anyway so `lang_for_ext` names the format
/// it serves, and so a future JSX-only rule has somewhere to live.
pub(crate) fn jsx_spec() -> LangSpec {
    js_spec()
}

/// The TS grammar rejects JSX outright — `<div />` in a return is a parse
/// error that swallows the surrounding declaration — and TSX needs its own
/// grammar, which also reads `<T>(x: T) => x` as a generic arrow rather than a
/// type assertion. The query is identical: TSX is TS plus JSX expressions.
pub(crate) fn tsx_spec() -> LangSpec {
    LangSpec {
        language: tree_sitter_typescript::LANGUAGE_TSX.into(),
        ..ts_spec()
    }
}

#[cfg(test)]
mod tests {
    use crate::formats::code::testutil::{assert_deterministic, assert_has};

    const TSX: &str = "import { useState } from \"react\";\nimport { Card } from \"./Card\";\n\nexport function Counter({ start }: { start: number }): JSX.Element {\n  const [n, setN] = useState(start);\n  return <Card title=\"count\" onClick={() => setN(n + 1)}>{n}</Card>;\n}\n\nexport class Panel extends Component<Props> {\n  render() {\n    return <div className=\"panel\" />;\n  }\n}\n\nfunction Hidden() {\n  return <span />;\n}\n";

    #[test]
    fn tsx_reads_a_generic_arrow_as_a_generic_not_a_cast() {
        // Under the plain TS grammar `<T,>` opens a type assertion; under TSX
        // it is a type parameter list. Either way the export after it must
        // still be found, which only holds if the file parsed.
        let src = "export const id = <T,>(x: T) => x;\nexport function after(): number {\n  return <b>{1}</b> ? 1 : 0;\n}\n";
        let e = crate::extract("id.tsx", src);
        assert_has(&e.symbols, "export function after(): number");
    }

    #[test]
    fn a_leading_comment_documents_a_component_file() {
        let src = "// Renders the cart badge.\nexport function Badge() {\n  return <b />;\n}\n";
        let e = crate::extract("Badge.tsx", src);
        assert_eq!(e.summary.as_deref(), Some("Renders the cart badge."));
    }

    #[test]
    fn extraction_is_deterministic() {
        assert_deterministic("src/Counter.tsx", TSX);
    }
}
