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
    use crate::formats::code::testutil::{assert_has, assert_lacks};
    use crate::formats::code::CodeExtractor;
    use crate::formats::Extractor;
    use crate::model::SourceKind;

    const TSX: &str = "import { useState } from \"react\";\nimport { Card } from \"./Card\";\n\nexport function Counter({ start }: { start: number }): JSX.Element {\n  const [n, setN] = useState(start);\n  return <Card title=\"count\" onClick={() => setN(n + 1)}>{n}</Card>;\n}\n\nexport class Panel extends Component<Props> {\n  render() {\n    return <div className=\"panel\" />;\n  }\n}\n\nfunction Hidden() {\n  return <span />;\n}\n";

    #[test]
    fn tsx_exports_survive_jsx_in_their_bodies() {
        let e = CodeExtractor.extract("src/Counter.tsx", TSX);
        assert_has(
            &e.symbols,
            "export function Counter({ start }: { start: number }): JSX.Element",
        );
        assert_has(&e.symbols, "export class Panel extends Component<Props>");
        assert_lacks(&e.symbols, "Hidden");
        // No JSX element is ever part of a signature: the cut stops at the body.
        assert_lacks(&e.symbols, "<Card");
        assert_lacks(&e.symbols, "<div");
        assert_eq!(e.imports, vec!["./Card", "react"]);
        assert_eq!(e.defined, vec!["Counter", "Panel"]);
        assert_eq!(
            e.kind,
            SourceKind::Code {
                lang: "typescript".into()
            }
        );
    }

    #[test]
    fn tsx_reads_a_generic_arrow_as_a_generic_not_a_cast() {
        // Under the plain TS grammar `<T,>` opens a type assertion; under TSX
        // it is a type parameter list. Either way the export after it must
        // still be found, which only holds if the file parsed.
        let src = "export const id = <T,>(x: T) => x;\nexport function after(): number {\n  return <b>{1}</b> ? 1 : 0;\n}\n";
        let e = CodeExtractor.extract("id.tsx", src);
        assert_has(&e.symbols, "export function after(): number");
    }

    #[test]
    fn jsx_exports_and_imports() {
        let src = "import Button from \"./Button.jsx\";\n\nexport function Toolbar({ items }) {\n  return (\n    <nav>\n      {items.map((i) => <Button key={i.id} {...i} />)}\n    </nav>\n  );\n}\n\nfunction local() {\n  return <i />;\n}\n";
        let e = CodeExtractor.extract("src/Toolbar.jsx", src);
        assert_eq!(e.symbols, vec!["export function Toolbar({ items })"]);
        assert_eq!(e.imports, vec!["./Button.jsx"]);
        assert_eq!(
            e.kind,
            SourceKind::Code {
                lang: "javascript".into()
            }
        );
    }

    #[test]
    fn a_leading_comment_documents_a_component_file() {
        let src = "// Renders the cart badge.\nexport function Badge() {\n  return <b />;\n}\n";
        let e = CodeExtractor.extract("Badge.tsx", src);
        assert_eq!(e.summary.as_deref(), Some("Renders the cart badge."));
    }

    #[test]
    fn index_tsx_and_index_jsx_are_named_for_their_directory() {
        let e = CodeExtractor.extract("src/components/index.tsx", "export function A() {}\n");
        assert_eq!(e.name, "Components");
        let e = CodeExtractor.extract("src/widgets/index.jsx", "export function B() {}\n");
        assert_eq!(e.name, "Widgets");
    }

    #[test]
    fn extraction_is_deterministic() {
        let a = CodeExtractor.extract("src/Counter.tsx", TSX);
        let b = CodeExtractor.extract("src/Counter.tsx", TSX);
        assert_eq!(a.symbols, b.symbols);
        assert_eq!(a.imports, b.imports);
        assert_eq!(a.summary, b.summary);
    }
}
