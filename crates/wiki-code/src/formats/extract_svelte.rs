//! Svelte components: the top-level `<script>` blocks (instance and
//! `context="module"` / `module`) are extracted by the existing JS/TS specs,
//! chosen by `lang`; markup and `<style>` contribute nothing.
//!
//! Same blank-in-place view as `extract_vue`, whose scanner this reuses. The
//! difference is the top level: a Vue component's top level is a list of
//! blocks, a Svelte component's is markup. So every element but `<style>` is
//! skipped as nested markup — a `<script>` inside `<svelte:head>` belongs to
//! the rendered page, not the component — and a `{...}` expression is skipped
//! whole, since it may hold a `<` that opens no tag.
use super::code::{default_shape, LangSpec};
use super::extract_simple::js_spec;
use super::extract_vue::{script_view, ScriptView};

/// A `<script>` with no `lang` is JavaScript, extracted under the `svelte`
/// key itself — see `extract_vue::vue_spec`.
pub(crate) fn svelte_spec() -> LangSpec {
    LangSpec {
        lang_name: "svelte",
        shape: default_shape,
        ..js_spec()
    }
}

pub(crate) fn svelte_script(source: &str) -> ScriptView {
    script_view(
        source,
        "svelte",
        |name| !name.eq_ignore_ascii_case("style"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::code::testutil::{assert_deterministic, assert_has, item_lines};

    const COMPONENT: &str = "<script context=\"module\" lang=\"ts\">\n// The cart drawer.\nexport function preload(id: string): Promise<Cart> {\n  return fetchCart(id);\n}\n</script>\n\n<script lang=\"ts\">\nimport Item from \"./Item.svelte\";\nimport { cart } from \"$lib/stores\";\nexport let open = false;\n</script>\n\n<svelte:head>\n  <script src=\"/analytics.js\">export function NotOurs() {}</script>\n</svelte:head>\n\n<link rel=\"stylesheet\" href=\"/x.css\">\n{#if $cart.items.length < 10}\n  <p>{\"<script>\"}</p>\n{/if}\n<aside class:open>\n  {#each $cart.items as item}<Item {item} />{/each}\n</aside>\n\n<style>\n  aside { width: 20rem; }\n  /* export function NotStyle() {} */\n</style>\n";

    #[test]
    fn the_blanked_view_keeps_every_byte_offset() {
        let view = svelte_script(COMPONENT);
        assert_eq!(view.ext, "ts");
        assert_eq!(view.text.len(), COMPONENT.len());
        let at = COMPONENT.find("export function preload").unwrap();
        assert_eq!(&view.text[at..at + 15], "export function");
        assert!(!view.text.contains("analytics"));
        assert!(!view.text.contains("<aside"));
    }

    #[test]
    fn a_script_after_void_elements_and_expressions_is_still_found() {
        // `<meta>` never closes and `{a < b}` opens no tag: either one, read
        // as an open element, would swallow the script that follows.
        let src = "<meta name=\"x\">\n<img src=\"a.png\">\n{a < b ? \"}\" : c}\n<p>{x}</p>\n<script>\nexport function late() {}\n</script>\n";
        let e = crate::extract("Late.svelte", src);
        assert_eq!(svelte_script(src).ext, "svelte");
        assert_has(&e.symbols, "export function late()");
    }

    #[test]
    fn svelte_5_module_attribute_and_tsx_free_scripts() {
        let src = "<script module>\nexport class Store {}\n</script>\n<script>\nlet { name } = $props();\n</script>\n<h1>{name}</h1>\n";
        let e = crate::extract("Store.svelte", src);
        assert_eq!(e.symbols, vec!["export class Store"]);
        assert_eq!(e.defined, vec!["Store"]);
    }

    #[test]
    fn an_unknown_script_language_extracts_nothing() {
        let src = "<script lang=\"coffee\">\nexport function looksLikeJs() {}\n</script>\n";
        let e = crate::extract("Legacy.svelte", src);
        assert!(e.symbols.is_empty(), "{:?}", e.symbols);
    }

    #[test]
    fn extraction_is_deterministic() {
        assert_deterministic("CartDrawer.svelte", COMPONENT);
    }

    #[test]
    fn svelte_items_count_lines_from_the_top_of_the_file() {
        let e = crate::extract(
            "B.svelte",
            "<h1>x</h1>\n<script>\n  function inc() {}\n</script>\n",
        );
        assert_eq!(item_lines(&e.items), vec!["3-3 fn inc priv"]);
    }
}
