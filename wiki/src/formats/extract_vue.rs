//! Vue single-file components: the top-level `<script>` blocks are extracted
//! by the existing JS/TS specs, chosen by the block's `lang`; `<template>`,
//! `<style>` and custom blocks are markup and contribute nothing.
//!
//! The script is not cut out of the file. Every byte outside a script block's
//! content is blanked to a space instead (newlines kept), so the text handed
//! to tree-sitter has the same length and line layout as the source: a
//! signature is still a byte range of the file, and a `<script>` plus a
//! `<script setup>` parse as one program without being stitched together.
//!
//! The block scanner is shared with `extract_svelte`, whose components have
//! the same shape with a different idea of which top-level elements are
//! markup.
use super::code::LangSpec;
use super::extract_simple::js_spec;
use std::ops::Range;

/// A `<script>` with no `lang` is JavaScript, extracted under the `vue` key
/// itself so the registry entry is the one a plain-JS component really uses.
pub(crate) fn vue_spec() -> LangSpec {
    LangSpec {
        lang_name: "vue",
        ..js_spec()
    }
}

/// The code of a component, ready for the ordinary extraction pipeline.
pub(crate) struct ScriptView {
    /// The `CODE_EXTENSIONS` key whose spec extracts `text`.
    pub(crate) ext: &'static str,
    /// The component source with everything but script content blanked.
    pub(crate) text: String,
}

pub(crate) fn vue_script(source: &str) -> ScriptView {
    // Only `<template>` nests: a custom block (`<i18n>`, `<docs>`) is raw text
    // to its closing tag, exactly like `<style>`.
    script_view(
        source,
        "vue",
        |name| name.eq_ignore_ascii_case("template"),
        false,
    )
}

/// What the scanner needs to know about one component format.
///
/// `is_markup` names the top-level elements whose content is parsed as nested
/// markup, so a `<script>` inside one is not a component script; every other
/// top-level element is raw text up to its own closing tag. `braces` skips a
/// top-level `{...}` expression, where Svelte allows a `<` that is not a tag.
pub(crate) fn script_view(
    source: &str,
    default_ext: &'static str,
    is_markup: fn(&str) -> bool,
    braces: bool,
) -> ScriptView {
    let blocks = top_level_scripts(source, is_markup, braces);
    // The first block that states a language decides for the component: Vue
    // and Svelte both reject a component whose blocks disagree.
    let lang = blocks.iter().find_map(|b| attr_value(b.attrs, "lang"));
    let ext = match lang.map(str::to_ascii_lowercase).as_deref() {
        None | Some("js" | "javascript") => Some(default_ext),
        Some("ts" | "typescript") => Some("ts"),
        Some("tsx") => Some("tsx"),
        Some("jsx") => Some("jsx"),
        // CoffeeScript and friends: no grammar here, and parsing them as JS
        // would invent exports. The component keeps its page with no code.
        Some(_) => None,
    };
    let keep: Vec<Range<usize>> = if ext.is_some() {
        blocks.into_iter().map(|b| b.content).collect()
    } else {
        Vec::new()
    };
    ScriptView {
        ext: ext.unwrap_or(default_ext),
        text: blank_outside(source, &keep),
    }
}

/// One top-level `<script>`: its attribute text and its content's byte range.
struct ScriptBlock<'a> {
    attrs: &'a str,
    content: Range<usize>,
}

/// `source` with every byte outside `keep` replaced by a space, line breaks
/// kept. Every range boundary sits at an ASCII `<` or `>`, so a multi-byte
/// scalar is either copied whole or blanked whole and the result stays UTF-8.
fn blank_outside(source: &str, keep: &[Range<usize>]) -> String {
    let src = source.as_bytes();
    let mut out: Vec<u8> = src
        .iter()
        .map(|&b| if matches!(b, b'\n' | b'\r') { b } else { b' ' })
        .collect();
    for r in keep {
        out[r.clone()].copy_from_slice(&src[r.clone()]);
    }
    String::from_utf8(out).expect("blanking replaces whole scalars with ASCII")
}

fn starts_with_ci(s: &[u8], prefix: &str) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// HTML elements that never take a closing tag. Only consulted for Svelte,
/// whose top level is markup: without it a top-level `<link>` would wait for a
/// `</link>` that never comes and swallow the script after it.
fn is_void(name: &str) -> bool {
    [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ]
    .iter()
    .any(|v| v.eq_ignore_ascii_case(name))
}

/// An opening tag at `pos` (which holds `<`): `(name, attrs, end, self_closing)`
/// with `end` just past its `>`. `None` when `<` does not open a tag — a name
/// must start with a letter and end at whitespace, `/` or `>`, so `a<b}` in a
/// Svelte expression is not one.
fn open_tag(src: &str, pos: usize) -> Option<(&str, &str, usize, bool)> {
    let bytes = src.as_bytes();
    let name_start = pos + 1;
    if !bytes.get(name_start)?.is_ascii_alphabetic() {
        return None;
    }
    let mut i = name_start;
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'.' | b'_'))
    {
        i += 1;
    }
    if !bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'>'))
    {
        return None;
    }
    let name = &src[name_start..i];
    let attrs_start = i;
    // Quotes and Svelte's `{...}` attribute values may both hold a `>`.
    let (mut quote, mut depth) = (None, 0usize);
    while let Some(&b) = bytes.get(i) {
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'{') => depth += 1,
            (None, b'}') => depth = depth.saturating_sub(1),
            (None, b'>') if depth == 0 => {
                let attrs = &src[attrs_start..i];
                return Some((name, attrs, i + 1, attrs.trim_end().ends_with('/')));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The byte offset of the first `</name` at or after `from` that is followed
/// by whitespace or `>`, case-insensitively.
fn find_close(src: &str, name: &str, from: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut i = from;
    while let Some(off) = src.get(i..)?.find("</") {
        let at = i + off;
        let after = at + 2 + name.len();
        if starts_with_ci(&bytes[at + 2..], name)
            && bytes
                .get(after)
                .is_none_or(|b| b.is_ascii_whitespace() || *b == b'>')
        {
            return Some(at);
        }
        i = at + 2;
    }
    None
}

/// Just past the `>` that ends the tag beginning at `at`, or the end of input.
fn past_gt(src: &str, at: usize) -> usize {
    src[at..].find('>').map_or(src.len(), |o| at + o + 1)
}

/// Just past the `-->` closing the comment that opens at `at`.
fn past_comment(src: &str, at: usize) -> usize {
    src[at + 4..]
        .find("-->")
        .map_or(src.len(), |o| at + 4 + o + 3)
}

/// Just past the element named `name` whose opening tag ends at `from`,
/// counting nested same-name elements (`<template v-if>` inside the root
/// `<template>`). Unclosed markup runs to the end of input.
fn past_nested(src: &str, name: &str, from: usize) -> usize {
    let bytes = src.as_bytes();
    let (mut depth, mut i) = (1usize, from);
    while let Some(off) = src[i..].find('<') {
        let at = i + off;
        if bytes[at..].starts_with(b"<!--") {
            i = past_comment(src, at);
        } else if bytes.get(at + 1) == Some(&b'/') {
            let close = find_close(src, name, at) == Some(at);
            i = past_gt(src, at);
            if close {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
        } else if let Some((n, _, end, self_closing)) = open_tag(src, at) {
            if n.eq_ignore_ascii_case(name) && !self_closing {
                depth += 1;
            }
            i = end;
        } else {
            i = at + 1;
        }
    }
    src.len()
}

/// Just past the `{...}` expression opening at `at`. A string may hold a
/// brace, so quotes are skipped whole.
fn past_braces(src: &str, at: usize) -> usize {
    let (mut depth, mut quote) = (0usize, None);
    for (i, b) in src.bytes().enumerate().skip(at) {
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (None, b'"' | b'\'' | b'`') => quote = Some(b),
            (None, b'{') => depth += 1,
            (None, b'}') => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
    }
    src.len()
}

/// The component's own `<script>` blocks, in source order. A script nested
/// inside markup (`<svelte:head><script src=…>`) belongs to the rendered page,
/// not to the component, and is skipped with the markup around it.
fn top_level_scripts(src: &str, is_markup: fn(&str) -> bool, braces: bool) -> Vec<ScriptBlock<'_>> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if braces && b == b'{' {
            i = past_braces(src, i);
            continue;
        }
        if b != b'<' {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"<!--") {
            i = past_comment(src, i);
            continue;
        }
        let Some((name, attrs, end, self_closing)) = open_tag(src, i) else {
            // A stray closing tag, or a `<` that opens nothing.
            i += 1;
            continue;
        };
        i = if self_closing || (braces && is_void(name)) {
            end
        } else if name.eq_ignore_ascii_case("script") {
            let close = find_close(src, "script", end).unwrap_or(src.len());
            out.push(ScriptBlock {
                attrs,
                content: end..close,
            });
            past_gt(src, close)
        } else if is_markup(name) {
            past_nested(src, name, end)
        } else {
            find_close(src, name, end).map_or(src.len(), |at| past_gt(src, at))
        };
    }
    out
}

/// The value of attribute `name` in a tag's attribute text: quoted, unquoted,
/// or `None` when the attribute is absent or bare (`<script setup>`).
fn attr_value<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let bytes = attrs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'/')
        {
            i += 1;
        }
        let key_start = i;
        while bytes
            .get(i)
            .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'/'))
        {
            i += 1;
        }
        let key = &attrs[key_start..i];
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let value = if bytes.get(i) == Some(&b'=') {
            i += 1;
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            let (start, stop) = if let Some(&q @ (b'"' | b'\'')) = bytes.get(i) {
                let s = i + 1;
                (s, attrs[s..].find(q as char).map_or(attrs.len(), |o| s + o))
            } else {
                let end = attrs[i..].find(|c: char| c.is_ascii_whitespace());
                (i, end.map_or(attrs.len(), |o| i + o))
            };
            i = stop + 1;
            Some(&attrs[start..stop])
        } else {
            None
        };
        if key.eq_ignore_ascii_case(name) {
            return value;
        }
        if key.is_empty() && value.is_none() {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::code::testutil::{
        assert_deterministic, assert_has, assert_lacks, item_lines,
    };
    use crate::formats::code::CodeExtractor;
    use crate::formats::{Extractor, Registry};
    use crate::model::SourceKind;
    use std::collections::BTreeMap;

    const SFC: &str = "<template>\n  <div class=\"card\">\n    <template v-if=\"open\"><slot /></template>\n    <pre>export function NotCode() {}</pre>\n  </div>\n</template>\n\n<script lang=\"ts\">\n// The product card.\nimport { defineComponent } from \"vue\";\nimport Price from \"./Price.vue\";\nexport function formatPrice(cents: number): string {\n  return (cents / 100).toFixed(2);\n}\nexport default defineComponent({ components: { Price } });\n</script>\n\n<script setup lang=\"ts\">\nimport { useCart } from \"@/stores/cart\";\nconst cart = useCart();\n</script>\n\n<style scoped>\n.card { color: red; }\n/* export function NotStyle() {} */\n</style>\n";

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
    fn the_blanked_view_keeps_every_byte_offset() {
        let view = vue_script(SFC);
        assert_eq!(view.ext, "ts");
        assert_eq!(view.text.len(), SFC.len());
        assert_eq!(view.text.lines().count(), SFC.lines().count());
        let at = SFC.find("export function formatPrice").unwrap();
        assert_eq!(&view.text[at..at + 15], "export function");
        // The same text inside the template's `<pre>` is blanked.
        let pre = SFC.find("export function NotCode").unwrap();
        assert_eq!(view.text[pre..pre + 15].trim(), "");
        assert!(!view.text.contains("<template>"));
        assert!(!view.text.contains(".card"));
    }

    #[test]
    fn a_script_without_lang_is_javascript() {
        let src = "<script>\nimport api from './api.js'\nexport class Store {}\n</script>\n<template><p>{{ x }}</p></template>\n";
        let view = vue_script(src);
        assert_eq!(view.ext, "vue");
        let e = CodeExtractor.extract("Store.vue", src);
        assert_has(&e.symbols, "export class Store");
        assert_eq!(e.imports, vec!["./api.js"]);
    }

    #[test]
    fn tsx_and_jsx_scripts_use_their_own_grammar() {
        let src = "<script lang=\"tsx\">\nexport function Row(): JSX.Element {\n  return <tr />;\n}\n</script>\n";
        assert_eq!(vue_script(src).ext, "tsx");
        let e = CodeExtractor.extract("Row.vue", src);
        assert_has(&e.symbols, "export function Row(): JSX.Element");
        assert_eq!(vue_script("<script lang='jsx'></script>").ext, "jsx");
        assert_eq!(vue_script("<script lang=TS></script>").ext, "ts");
    }

    #[test]
    fn an_unknown_script_language_extracts_nothing() {
        let src = "<script lang=\"coffee\">\nexport function looksLikeJs() {}\n</script>\n";
        let e = CodeExtractor.extract("Legacy.vue", src);
        assert!(e.symbols.is_empty(), "{:?}", e.symbols);
        assert_eq!(e.kind, SourceKind::Code { lang: "vue".into() });
    }

    #[test]
    fn markup_only_component_has_a_page_and_no_code() {
        let src = "<template>\n  <script>export function Inline() {}</script>\n</template>\n";
        let e = CodeExtractor.extract("Static.vue", src);
        assert!(e.symbols.is_empty(), "{:?}", e.symbols);
        assert!(e.imports.is_empty(), "{:?}", e.imports);
    }

    #[test]
    fn custom_blocks_and_comments_are_raw_text() {
        let src = "<!-- <script>export function InComment() {}</script> -->\n<i18n>\n{ \"a\": \"<script>\" }\n</i18n>\n<script>\nexport function real() {}\n</script>\n";
        let e = CodeExtractor.extract("I18n.vue", src);
        assert_eq!(e.symbols, vec!["export function real()"]);
    }

    #[test]
    fn attr_value_reads_every_quoting_style() {
        assert_eq!(attr_value(" setup lang=\"ts\"", "lang"), Some("ts"));
        assert_eq!(attr_value(" lang='ts' setup", "lang"), Some("ts"));
        assert_eq!(attr_value(" lang = ts", "lang"), Some("ts"));
        assert_eq!(attr_value(" setup", "lang"), None);
        assert_eq!(attr_value(" xlang=\"ts\"", "lang"), None);
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

    #[test]
    fn extraction_is_deterministic() {
        assert_deterministic("ProductCard.vue", SFC);
    }

    #[test]
    fn component_items_count_lines_from_the_top_of_the_file() {
        let src = "<template>\n  <div/>\n</template>\n<script setup lang=\"ts\">\n/** Counts clicks. */\nfunction count(n: number) {\n  return n + 1;\n}\n</script>\n";
        let e = CodeExtractor.extract("C.vue", src);
        assert_eq!(item_lines(&e.items), vec!["5-8 fn count priv"]);
        assert_eq!(e.items[0].doc.as_deref(), Some("Counts clicks."));
    }
}
