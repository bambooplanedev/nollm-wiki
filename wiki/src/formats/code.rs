//! The code page: `wiki_code::extract`'s result as an `Entity`.

use crate::formats::Extractor;
use crate::model::{slugify, Entity, SourceKind};
pub(crate) use wiki_code::support::{is_shell, module_stem, source_rank, validate_queries};

pub struct CodeExtractor;

impl Extractor for CodeExtractor {
    fn extensions(&self) -> &[&str] {
        wiki_code::support::CODE_EXTENSIONS
    }

    fn extract(&self, rel_path: &str, text: &str) -> Entity {
        let f = wiki_code::extract(rel_path, text);
        Entity {
            id: slugify(&f.name),
            name: f.name,
            body: f.body,
            kind: SourceKind::Code { lang: f.lang },
            summary: f.summary,
            symbols: f.symbols,
            imports: f.imports,
            defined: f.defined,
            methods: f.methods,
            items: f.items,
            ..Entity::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::Registry;
    use std::collections::BTreeMap;

    /// The exact signature `want` must be among `symbols`. A copy of
    /// `wiki_code`'s test helper, which is not visible outside that crate.
    fn assert_has(symbols: &[String], want: &str) {
        assert!(
            symbols.iter().any(|s| s == want),
            "want {want:?} in {symbols:?}"
        );
    }

    /// No signature may so much as mention `unwanted`.
    fn assert_lacks(symbols: &[String], unwanted: &str) {
        assert!(
            !symbols.iter().any(|s| s.contains(unwanted)),
            "{unwanted:?} must not appear in {symbols:?}"
        );
    }

    const TSX: &str = "import { useState } from \"react\";\nimport { Card } from \"./Card\";\n\nexport function Counter({ start }: { start: number }): JSX.Element {\n  const [n, setN] = useState(start);\n  return <Card title=\"count\" onClick={() => setN(n + 1)}>{n}</Card>;\n}\n\nexport class Panel extends Component<Props> {\n  render() {\n    return <div className=\"panel\" />;\n  }\n}\n\nfunction Hidden() {\n  return <span />;\n}\n";

    const COMPONENT: &str = "<script context=\"module\" lang=\"ts\">\n// The cart drawer.\nexport function preload(id: string): Promise<Cart> {\n  return fetchCart(id);\n}\n</script>\n\n<script lang=\"ts\">\nimport Item from \"./Item.svelte\";\nimport { cart } from \"$lib/stores\";\nexport let open = false;\n</script>\n\n<svelte:head>\n  <script src=\"/analytics.js\">export function NotOurs() {}</script>\n</svelte:head>\n\n<link rel=\"stylesheet\" href=\"/x.css\">\n{#if $cart.items.length < 10}\n  <p>{\"<script>\"}</p>\n{/if}\n<aside class:open>\n  {#each $cart.items as item}<Item {item} />{/each}\n</aside>\n\n<style>\n  aside { width: 20rem; }\n  /* export function NotStyle() {} */\n</style>\n";

    const SFC: &str = "<template>\n  <div class=\"card\">\n    <template v-if=\"open\"><slot /></template>\n    <pre>export function NotCode() {}</pre>\n  </div>\n</template>\n\n<script lang=\"ts\">\n// The product card.\nimport { defineComponent } from \"vue\";\nimport Price from \"./Price.vue\";\nexport function formatPrice(cents: number): string {\n  return (cents / 100).toFixed(2);\n}\nexport default defineComponent({ components: { Price } });\n</script>\n\n<script setup lang=\"ts\">\nimport { useCart } from \"@/stores/cart\";\nconst cart = useCart();\n</script>\n\n<style scoped>\n.card { color: red; }\n/* export function NotStyle() {} */\n</style>\n";

    #[test]
    fn uppercase_extension_still_extracts() {
        let e = CodeExtractor.extract("src/Book.H", "class Book {};\n");
        match &e.kind {
            crate::model::SourceKind::Code { lang } => assert_eq!(lang, "cpp"),
            other => panic!("expected Code kind, got {other:?}"),
        }
    }

    #[test]
    fn a_directory_module_is_named_for_its_directory() {
        // Rust's `mod.rs` and Python's `__init__.py` carry no identity in
        // their own filename — both languages refer to such a module by its
        // DIRECTORY. Naming the page "Mod"/"Init" made it unreachable: an
        // importer's body says `mod common; use common::helper`, which never
        // contains the page's own name, so neither the phrase index nor
        // import resolution could ever link it. Measured on this repo,
        // `tests/common/mod.rs` was a permanent orphan.
        let e = CodeExtractor.extract("tests/common/mod.rs", "pub fn helper() -> u8 { 0 }\n");
        assert_eq!(e.name, "Common");
        assert_eq!(e.id, "common");

        let e = CodeExtractor.extract("pkg/__init__.py", "def exported():\n    pass\n");
        assert_eq!(e.name, "Pkg");
        assert_eq!(e.id, "pkg");
    }

    #[test]
    fn a_directory_module_at_the_corpus_root_keeps_its_own_stem() {
        // Nothing to borrow a name from, so the file's own stem is kept.
        let e = CodeExtractor.extract("mod.rs", "pub fn f() {}\n");
        assert_eq!(e.name, "Mod");
        let e = CodeExtractor.extract("__init__.py", "def f():\n    pass\n");
        assert_eq!(e.name, "Init");
    }

    #[test]
    fn an_ordinary_file_is_still_named_for_itself_not_its_directory() {
        // Guards the overcorrection: only the two directory-module markers
        // borrow the parent name.
        let e = CodeExtractor.extract("src/formats/code.rs", "pub fn f() {}\n");
        assert_eq!(e.name, "Code");
        let e = CodeExtractor.extract("src/models.py", "def f():\n    pass\n");
        assert_eq!(e.name, "Models");
    }

    #[test]
    fn a_mod_file_in_another_language_is_not_treated_as_a_directory_module() {
        // `mod` is only a directory-module marker for Rust; a `mod.py` is an
        // ordinary module named `mod`.
        let e = CodeExtractor.extract("pkg/mod.py", "def f():\n    pass\n");
        assert_eq!(e.name, "Mod");
    }

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
    fn index_tsx_and_index_jsx_are_named_for_their_directory() {
        let e = CodeExtractor.extract("src/components/index.tsx", "export function A() {}\n");
        assert_eq!(e.name, "Components");
        let e = CodeExtractor.extract("src/widgets/index.jsx", "export function B() {}\n");
        assert_eq!(e.name, "Widgets");
    }

    #[test]
    fn rust_signatures_gated_and_imports() {
        let src = "//! Module docs.\nuse crate::graph::build;\npub fn render_page(x: i32) -> String { String::new() }\nfn private_helper() {}\n";
        let e = CodeExtractor.extract("src/rewrite.rs", src);
        match &e.kind {
            crate::model::SourceKind::Code { lang } => assert_eq!(lang, "rust"),
            _ => panic!("expected Code kind"),
        }
        assert_has(&e.symbols, "pub fn render_page(x: i32) -> String");
        assert_lacks(&e.symbols, "private_helper");
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert_eq!(e.summary.as_deref(), Some("Module docs."));
    }

    #[test]
    fn both_scripts_are_extracted_and_markup_is_not() {
        let e = CodeExtractor.extract("src/lib/CartDrawer.svelte", COMPONENT);
        assert_eq!(
            e.symbols,
            vec!["export function preload(id: string): Promise<Cart>"]
        );
        assert_lacks(&e.symbols, "NotOurs");
        assert_lacks(&e.symbols, "NotStyle");
        assert_eq!(e.imports, vec!["$lib/stores", "./Item.svelte"]);
        assert_eq!(e.summary.as_deref(), Some("The cart drawer."));
        assert_eq!(
            e.kind,
            SourceKind::Code {
                lang: "svelte".into()
            }
        );
        assert_eq!(e.body, COMPONENT);
    }

    #[test]
    fn the_registry_routes_svelte_and_imports_draw_edges() {
        let reg = Registry::with_defaults();
        assert!(reg.handles("src/App.svelte"));
        let app = "<script>\n  import CartDrawer from './lib/CartDrawer.svelte';\n</script>\n<CartDrawer open />\n";
        let entities: BTreeMap<String, crate::model::Entity> = [
            ("src/App.svelte", app),
            ("src/lib/CartDrawer.svelte", COMPONENT),
        ]
        .into_iter()
        .map(|(p, s)| {
            let e = reg.extract(p, s.as_bytes()).unwrap();
            (e.id.clone(), e)
        })
        .collect();
        let g = crate::graph::build_graph(&entities);
        assert!(
            g.edges["app"].outgoing.contains("cartdrawer"),
            "edges: {:?}",
            g.edges
        );
    }

    #[test]
    fn script_blocks_are_extracted_and_markup_is_not() {
        let e = CodeExtractor.extract("src/components/ProductCard.vue", SFC);
        assert_eq!(
            e.symbols,
            vec!["export function formatPrice(cents: number): string"]
        );
        assert_lacks(&e.symbols, "NotCode");
        assert_lacks(&e.symbols, "NotStyle");
        // Both blocks contribute imports, as one program.
        assert_eq!(e.imports, vec!["./Price.vue", "@/stores/cart", "vue"]);
        assert_eq!(e.defined, vec!["formatPrice"]);
        assert_eq!(e.summary.as_deref(), Some("The product card."));
        assert_eq!(e.id, "productcard");
        assert_eq!(e.kind, SourceKind::Code { lang: "vue".into() });
        // The page body is still the whole component.
        assert_eq!(e.body, SFC);
    }

    #[test]
    fn an_unknown_script_language_extracts_nothing() {
        let src = "<script lang=\"coffee\">\nexport function looksLikeJs() {}\n</script>\n";
        let e = CodeExtractor.extract("Legacy.vue", src);
        assert!(e.symbols.is_empty(), "{:?}", e.symbols);
        assert_eq!(e.kind, SourceKind::Code { lang: "vue".into() });
    }

    #[test]
    fn the_registry_routes_vue_and_imports_draw_edges() {
        let reg = Registry::with_defaults();
        assert!(reg.handles("src/App.vue"));
        let app = "<script setup>\nimport ProductCard from './components/ProductCard.vue'\n</script>\n<template><ProductCard /></template>\n";
        let entities: BTreeMap<String, crate::model::Entity> = [
            ("src/App.vue", app),
            ("src/components/ProductCard.vue", SFC),
        ]
        .into_iter()
        .map(|(p, s)| {
            let e = reg.extract(p, s.as_bytes()).unwrap();
            (e.id.clone(), e)
        })
        .collect();
        let g = crate::graph::build_graph(&entities);
        assert!(
            g.edges["app"].outgoing.contains("productcard"),
            "edges: {:?}",
            g.edges
        );
    }
}
