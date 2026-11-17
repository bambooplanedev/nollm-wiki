//! Tests for `code`.
use super::*;
use crate::formats::Registry;
use std::collections::BTreeMap;

/// No signature may so much as mention `unwanted`.
fn assert_lacks(symbols: &[String], unwanted: &str) {
    assert!(
        !symbols.iter().any(|s| s.contains(unwanted)),
        "{unwanted:?} must not appear in {symbols:?}"
    );
}

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
fn a_directory_module_at_the_corpus_root_keeps_its_own_stem() {
    // Nothing to borrow a name from, so the file's own stem is kept.
    let e = CodeExtractor.extract("mod.rs", "pub fn f() {}\n");
    assert_eq!(e.name, "Mod");
    let e = CodeExtractor.extract("__init__.py", "def f():\n    pass\n");
    assert_eq!(e.name, "Init");
}

#[test]
fn a_mod_file_in_another_language_is_not_treated_as_a_directory_module() {
    // `mod` is only a directory-module marker for Rust; a `mod.py` is an
    // ordinary module named `mod`.
    let e = CodeExtractor.extract("pkg/mod.py", "def f():\n    pass\n");
    assert_eq!(e.name, "Mod");
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
