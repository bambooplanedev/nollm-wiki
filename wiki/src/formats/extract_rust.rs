//! Rust extraction: bare-`pub` visibility gating, owner qualification through
//! `impl`/`trait` scopes, and `#[cfg(test)]` module stripping.
use super::code::{keep_all, LangSpec};
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

/// Rust only. `(visibility_modifier)` also covers `pub(crate)`, `pub(super)`,
/// and `pub(in path)`, none of which leave the crate; bare `pub` is the only
/// one that belongs in `## Exports`.
fn keep_bare_pub(vis: &str) -> bool {
    vis == "pub"
}

/// The owner of a Rust definition, rendered as it will appear in the
/// signature:
///
///   * inherent `impl` — the type name with generic arguments stripped, so
///     `impl<T> Holder<T>` yields `Holder::get`, valid Rust that sorts beside
///     the type's other methods;
///   * trait `impl` — `<Type as Trait>`, Rust's own disambiguation syntax. The
///     trait *must* be part of the owner: `impl Display for Foo` and
///     `impl Debug for Foo` both define `fn fmt(&self, …) -> Result`, which
///     collapse to a single line under `dedup` if only the type is named. It
///     also keeps the full type text, so `impl Encode for Vec<u8>` and
///     `… for Vec<u16>` stay distinct, and it renders exotic targets as valid
///     paths (`<&Foo as Trait>::m`).
///   * `trait` declaration — the trait's own name.
fn rust_owner(def: Node, text: &str) -> Option<String> {
    let parent = def.parent()?;
    if parent.kind() != "declaration_list" {
        return None;
    }
    let holder = parent.parent()?;
    match holder.kind() {
        "impl_item" => {
            let ty = text.get(holder.child_by_field_name("type")?.byte_range())?;
            match holder.child_by_field_name("trait") {
                Some(tr) => Some(format!("<{} as {}>", ty, text.get(tr.byte_range())?)),
                None => Some(ty.split('<').next().unwrap_or(ty).trim().to_string()),
            }
        }
        "trait_item" => text
            .get(holder.child_by_field_name("name")?.byte_range())
            .map(str::to_string),
        _ => None,
    }
}

/// True when `node` is reachable from the file root through module and
/// type-definition scopes only. Reaching a `block` means the item lives inside
/// a function body — a fixture `impl` written in a helper is not part of the
/// module's public surface, and tree-sitter queries match at any depth.
fn rust_module_level(mut node: Node) -> bool {
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "source_file" | "mod_item" | "declaration_list" | "impl_item" | "trait_item" => {
                node = parent
            }
            _ => return false,
        }
    }
    true
}

pub(crate) fn rust_spec() -> LangSpec {
    LangSpec {
        lang_name: "rust",
        language: tree_sitter_rust::LANGUAGE.into(),
        // Requiring `(visibility_modifier)` excludes private items
        // structurally; `vis_filter` then drops the restricted forms
        // (`pub(crate)` and friends) that the node kind also covers.
        // The trait-impl patterns deliberately capture no `@vis`: rustc
        // rejects a visibility modifier there, and those items are public
        // through the trait. The trait-declaration patterns gate on the
        // trait's own visibility instead of the method's.
        query_src: r#"
                (function_item (visibility_modifier) @vis name: (identifier) @name) @def
                (struct_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (enum_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (trait_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (const_item (visibility_modifier) @vis name: (identifier) @name) @def
                (static_item (visibility_modifier) @vis name: (identifier) @name) @def
                (type_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (impl_item trait: (_) type: (_)) @def
                (impl_item trait: (_) body: (declaration_list (function_item name: (identifier) @name) @def))
                (impl_item trait: (_) body: (declaration_list (type_item name: (type_identifier) @name) @def))
                (impl_item trait: (_) body: (declaration_list (const_item name: (identifier) @name) @def))
                (trait_item (visibility_modifier) @vis body: (declaration_list (function_signature_item name: (identifier) @name) @def))
                (trait_item (visibility_modifier) @vis body: (declaration_list (function_item name: (identifier) @name) @def))
                (use_declaration argument: (_) @import)
            "#,
        name_filter: keep_all,
        vis_filter: keep_bare_pub,
        strip_trailing: &[';', '='],
        owner_of: rust_owner,
        owner_sep: "::",
        def_filter: rust_module_level,
    }
}

/// Remove `#[cfg(test)]`-annotated `mod` items from Rust source, replacing
/// each with a one-line omission marker (`// [tests omitted: mod <name>,
/// <N> lines]`). The spliced span starts at the first attribute in the
/// contiguous run of attributes directly above the `mod` (so `#[cfg(test)]`
/// itself is removed) and ends at the module's closing brace; `<N>` is that
/// span's line count. Returns `None` when there is nothing to strip or the
/// source fails to parse — the caller keeps the raw text.
pub(crate) fn strip_rust_test_modules(text: &str) -> Option<String> {
    let language: Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(text, None)?;
    let query = Query::new(&language, "(mod_item) @m").ok()?;
    let mut cursor = QueryCursor::new();

    // (start, end, mod name) per cfg(test) module, at any nesting depth.
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    let matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    for m in matches {
        for cap in m.captures {
            let node = cap.node;
            // Walk the contiguous run of attribute_items directly above the
            // mod; the whole run is spliced when any of them is cfg(test).
            let mut start = node.start_byte();
            let mut is_test_mod = false;
            let mut prev = node.prev_named_sibling();
            while let Some(p) = prev {
                if p.kind() != "attribute_item" {
                    break;
                }
                let attr: String = text
                    .get(p.byte_range())
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                if attr == "#[cfg(test)]" {
                    is_test_mod = true;
                }
                start = p.start_byte();
                prev = p.prev_named_sibling();
            }
            if !is_test_mod {
                continue;
            }
            let name = node
                .child_by_field_name("name")
                .and_then(|n| text.get(n.byte_range()))
                .unwrap_or("?")
                .to_string();
            spans.push((start, node.end_byte(), name));
        }
    }
    if spans.is_empty() {
        return None;
    }
    spans.sort_by_key(|s| s.0);

    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for (start, end, name) in spans {
        if start < pos {
            continue; // nested inside a module already removed
        }
        let lines = text[start..end].lines().count();
        out.push_str(&text[pos..start]);
        out.push_str(&format!("// [tests omitted: mod {name}, {lines} lines]"));
        // The marker is a line comment. Anything that followed the module's
        // closing brace on the same line would otherwise be commented out —
        // cosmetic in `## Body`, but a silent loss of exports once symbols
        // are extracted from this text.
        let rest = &text[end..];
        if !rest.is_empty() && !rest.starts_with('\n') {
            out.push('\n');
        }
        pos = end;
    }
    out.push_str(&text[pos..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use crate::formats::code::CodeExtractor;
    use crate::formats::Extractor;

    #[test]
    fn rust_signatures_gated_and_imports() {
        let src = "//! Module docs.\nuse crate::graph::build;\npub fn render_page(x: i32) -> String { String::new() }\nfn private_helper() {}\n";
        let e = CodeExtractor.extract("src/rewrite.rs", src);
        match &e.kind {
            crate::model::SourceKind::Code { lang } => assert_eq!(lang, "rust"),
            _ => panic!("expected Code kind"),
        }
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub fn render_page(x: i32) -> String"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("private_helper")));
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert_eq!(e.summary.as_deref(), Some("Module docs."));
    }

    #[test]
    fn rust_restricted_visibility_is_not_an_export() {
        let src = "pub fn public_one() {}\npub(crate) fn crate_only() {}\npub(super) fn super_only() {}\npub(crate) struct CrateType;\nstruct Holder;\nimpl Holder {\n    pub fn kept(&self) {}\n    pub(crate) fn impl_crate_only(&self) {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols.iter().any(|s| s == "pub fn public_one()"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s.contains("kept")),
            "a bare-pub inherent method must survive: {:?}",
            e.symbols
        );
        for leaked in ["crate_only", "super_only", "CrateType", "impl_crate_only"] {
            assert!(
                !e.symbols.iter().any(|s| s.contains(leaked)),
                "{leaked} is not exported outside the crate: {:?}",
                e.symbols
            );
        }
    }

    #[test]
    fn rust_trait_visibility_gated() {
        let src = "pub trait Public {\n    fn m(&self);\n}\ntrait Private {\n    fn n(&self);\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols.iter().any(|s| s == "pub trait Public"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("Private")),
            "private trait leaked as export: {:?}",
            e.symbols
        );
    }

    #[test]
    fn rust_module_level_const_static_and_type_alias() {
        let src = "pub const CACHE_VERSION: u32 = 1;\npub static NAME: &str = \"x\";\npub type Pack = Vec<u8>;\nconst PRIVATE: u8 = 3;\n";
        let e = CodeExtractor.extract("t.rs", src);
        // A const's contract is its type; the literal stays visible in `## Body`,
        // exactly as a function's body does.
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub const CACHE_VERSION: u32"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "pub static NAME: &str"),
            "symbols: {:?}",
            e.symbols
        );
        // An alias without its target would be useless, and `type_item` has no
        // `value:` field, so nothing is cut.
        assert!(
            e.symbols.iter().any(|s| s == "pub type Pack = Vec<u8>"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("PRIVATE")),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn inherent_impl_methods_are_qualified_with_their_type() {
        let src = "pub struct Wiki;\nimpl Wiki {\n    pub fn search(&self, q: &str) -> Vec<Hit> { todo!() }\n    fn helper(&self) {}\n}\npub fn free_function() {}\n";
        let e = CodeExtractor.extract("query.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub fn Wiki::search(&self, q: &str) -> Vec<Hit>"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "pub fn free_function()"),
            "a top-level function must stay unqualified: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("helper")));
    }

    #[test]
    fn generic_inherent_impl_strips_type_arguments() {
        let src = "impl<T: Clone> Holder<T> {\n    pub fn get(&self) -> T { todo!() }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        // `Holder::get` is valid Rust and sorts beside the type's other
        // methods; `Holder<T>::get` would be neither.
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub fn Holder::get(&self) -> T"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn function_local_items_are_not_module_exports() {
        let src = "pub fn outer() {\n    pub struct Local;\n    impl Local {\n        pub fn hidden(&self) {}\n    }\n    pub fn nested() {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols.iter().any(|s| s == "pub fn outer()"),
            "symbols: {:?}",
            e.symbols
        );
        for leaked in ["Local", "hidden", "nested"] {
            assert!(
                !e.symbols.iter().any(|s| s.contains(leaked)),
                "{leaked} lives in a function body and is not module surface: {:?}",
                e.symbols
            );
        }
    }

    #[test]
    fn function_local_trait_impls_and_traits_are_not_exports() {
        let src = "pub fn outer() {\n    struct Local;\n    impl Display for Local {\n        fn fmt(&self) {}\n    }\n    pub trait Hidden {\n        fn h(&self);\n    }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_eq!(e.symbols, vec!["pub fn outer()".to_string()]);
    }

    #[test]
    fn generic_trait_impl_keeps_its_type_arguments() {
        let src = "impl<T: Clone> Display for Wrapper<T> {\n    fn fmt(&self) {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "impl<T: Clone> Display for Wrapper<T>"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn <Wrapper<T> as Display>::fmt(&self)"),
            "a trait impl keeps the full type, unlike an inherent impl: {:?}",
            e.symbols
        );
    }

    #[test]
    fn items_in_an_inline_module_stay_unqualified() {
        let src = "pub mod inner {\n    pub fn in_mod() {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols.iter().any(|s| s == "pub fn in_mod()"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn rust_test_module_stripped_with_marker() {
        let src = "//! Docs.\npub fn real() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn checks_real() {\n        real();\n    }\n}\n";
        let e = CodeExtractor.extract("src/thing.rs", src);
        assert!(
            !e.body.contains("checks_real"),
            "test fn text must be gone: {}",
            e.body
        );
        assert!(!e.body.contains("#[cfg(test)]"), "body: {}", e.body);
        // Removed span = the attribute line through the closing brace: 9 lines.
        assert!(
            e.body.contains("// [tests omitted: mod tests, 9 lines]"),
            "body: {}",
            e.body
        );
        assert!(e.body.contains("pub fn real()"), "body: {}", e.body);
    }

    #[test]
    fn rust_without_test_module_round_trips_byte_identical() {
        let src = "//! Docs.\nuse std::fmt;\npub fn only() {}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_eq!(e.body, src);
    }

    #[test]
    fn rust_two_test_modules_get_two_markers_in_source_order() {
        let src = "pub fn a() {}\n\n#[cfg(test)]\nmod early_tests {\n    #[test]\n    fn t1() {}\n}\n\npub fn b() {}\n\n#[cfg(test)]\nmod late_tests {\n    #[test]\n    fn t2() {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        let early = e.body.find("// [tests omitted: mod early_tests, 5 lines]");
        let late = e.body.find("// [tests omitted: mod late_tests, 5 lines]");
        assert!(early.is_some() && late.is_some(), "body: {}", e.body);
        assert!(
            early.unwrap() < late.unwrap(),
            "markers out of order: {}",
            e.body
        );
        assert!(!e.body.contains("fn t1") && !e.body.contains("fn t2"));
    }

    #[test]
    fn rust_entirely_test_module_body_is_just_the_marker() {
        let src = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_eq!(e.body.trim(), "// [tests omitted: mod tests, 5 lines]");
    }

    #[test]
    fn code_after_a_test_module_on_the_same_line_is_not_commented_out() {
        let src = "pub fn before() {}\n#[cfg(test)]\nmod tests { #[test] fn t() {} } pub fn after() -> u8 { 1 }\n";
        let e = CodeExtractor.extract("t.rs", src);
        let marker_line = e
            .body
            .lines()
            .find(|l| l.contains("[tests omitted"))
            .unwrap_or_else(|| panic!("no marker in body: {}", e.body));
        assert!(
            !marker_line.contains("pub fn after"),
            "code following the module was swallowed by the marker comment: {marker_line}"
        );
        // The body assertion above only catches the symptom; the stake is
        // that a same-line export doesn't silently vanish from the page's
        // exports once the newline separates it from the marker comment.
        assert!(
            e.symbols.iter().any(|s| s == "pub fn after() -> u8"),
            "export following the test module on the same line was lost: {:?}",
            e.symbols
        );
    }

    #[test]
    fn cfg_test_matches_modulo_whitespace_but_other_cfgs_do_not() {
        // Interior whitespace in the attribute still counts as cfg(test).
        let ws = "#[cfg( test )]\nmod tests {}\npub fn x() {}\n";
        let e = CodeExtractor.extract("a.rs", ws);
        assert!(
            e.body.contains("// [tests omitted: mod tests, 2 lines]"),
            "body: {}",
            e.body
        );

        // A different cfg is NOT a test module.
        let feature = "#[cfg(feature = \"extra\")]\nmod extra {}\npub fn x() {}\n";
        let e2 = CodeExtractor.extract("b.rs", feature);
        assert_eq!(e2.body, feature);
    }

    #[test]
    fn symbols_and_imports_come_from_the_test_stripped_source() {
        let src = "pub struct Fixture;\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    use tempfile::tempdir;\n\n    impl Default for Fixture {\n        fn default() -> Self { Fixture }\n    }\n\n    #[test]\n    fn t() {\n        let _ = tempdir();\n    }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            !e.imports.iter().any(|i| i == "super::*"),
            "test-only import leaked: {:?}",
            e.imports
        );
        assert!(
            !e.imports.iter().any(|i| i.contains("tempfile")),
            "test-only import leaked: {:?}",
            e.imports
        );
        assert!(
            e.symbols.iter().any(|s| s == "pub struct Fixture"),
            "real export lost: {:?}",
            e.symbols
        );
        // The fixture also plants an `impl Default for Fixture` inside the
        // `#[cfg(test)]` module — exactly the widening the module-level
        // guard exists to prevent. It must not survive stripping.
        assert!(
            !e.symbols.iter().any(|s| s.contains("Default")),
            "impl inside the stripped test module leaked as an export: {:?}",
            e.symbols
        );
    }

    #[test]
    fn trait_impl_emits_header_and_qualified_methods() {
        let src = "pub struct TextExtractor;\nimpl Extractor for TextExtractor {\n    fn extensions(&self) -> &[&str] { &[] }\n    fn extract(&self, p: &str) -> Entity { todo!() }\n}\n";
        let e = CodeExtractor.extract("text.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "impl Extractor for TextExtractor"),
            "symbols: {:?}",
            e.symbols
        );
        // Methods in a trait impl carry no visibility modifier — they are
        // public through the trait — so they must not be visibility-gated.
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn <TextExtractor as Extractor>::extract(&self, p: &str) -> Entity"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn two_traits_with_the_same_method_name_stay_distinct() {
        let src = "pub struct Foo;\nimpl Display for Foo {\n    fn fmt(&self) -> Result { todo!() }\n}\nimpl Debug for Foo {\n    fn fmt(&self) -> Result { todo!() }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn <Foo as Display>::fmt(&self) -> Result"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn <Foo as Debug>::fmt(&self) -> Result"),
            "dedup collapsed two distinct methods: {:?}",
            e.symbols
        );
    }

    #[test]
    fn same_trait_on_different_generic_arguments_stays_distinct() {
        let src = "impl Encode for Vec<u8> {\n    fn go(&self) -> u8 { 0 }\n}\nimpl Encode for Vec<u16> {\n    fn go(&self) -> u8 { 1 }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn <Vec<u8> as Encode>::go(&self) -> u8"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn <Vec<u16> as Encode>::go(&self) -> u8"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn exotic_impl_targets_render_as_valid_paths() {
        let src = "impl Trait for &Foo {\n    fn m(&self) {}\n}\nimpl Trait for (A, B) {\n    fn m(&self) {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn <&Foo as Trait>::m(&self)"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn <(A, B) as Trait>::m(&self)"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn associated_type_and_const_in_a_trait_impl_are_exported() {
        let src = "impl Iterator for Counter {\n    type Item = u32;\n    const FOO: u8 = 1;\n    fn next(&mut self) -> Option<u32> { None }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "type <Counter as Iterator>::Item = u32"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "const <Counter as Iterator>::FOO: u8"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn public_trait_declaration_lists_required_and_default_methods() {
        let src = "pub trait Extractor {\n    fn extensions(&self) -> &[&str];\n    fn helper(&self) -> u8 { 7 }\n}\ntrait Private {\n    fn n(&self);\n}\n";
        let e = CodeExtractor.extract("mod.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn Extractor::extensions(&self) -> &[&str]"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "fn Extractor::helper(&self) -> u8"),
            "symbols: {:?}",
            e.symbols
        );
        // Gated on the *trait's* visibility: a method inside a trait cannot
        // carry a modifier, but a private trait's methods are not exports.
        assert!(
            !e.symbols.iter().any(|s| s.contains("Private")),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn qualification_introduces_no_duplicate_symbols() {
        let src = "pub struct Wiki;\nimpl Wiki {\n    pub fn go(&self) {}\n}\nimpl Display for Wiki {\n    fn fmt(&self) {}\n}\npub fn go() {}\n";
        let e = CodeExtractor.extract("t.rs", src);
        // Asserting the exact list, not `dedup().len()`: extraction already
        // deduplicates, so a self-comparison would pass no matter what. An
        // inherent method is matched by the top-level `function_item` pattern
        // and qualified by the ancestor walk — never by a second pattern,
        // which would emit it twice and let `dedup` hide the bug.
        assert_eq!(
            e.symbols,
            vec![
                "fn <Wiki as Display>::fmt(&self)".to_string(),
                "impl Display for Wiki".to_string(),
                "pub fn Wiki::go(&self)".to_string(),
                "pub fn go()".to_string(),
                "pub struct Wiki".to_string(),
            ]
        );
    }

    #[test]
    fn summary_fallback_prefers_a_free_item_over_a_qualified_method() {
        let src = "pub struct TextExtractor;\nimpl Extractor for TextExtractor {\n    fn extensions(&self) -> &[&str] { &[] }\n}\n";
        let e = CodeExtractor.extract("text.rs", src);
        assert_eq!(e.summary.as_deref(), Some("pub struct TextExtractor"));
    }

    #[test]
    fn summary_fallback_uses_the_impl_header_when_no_free_item_exists() {
        let src = "impl Extractor for Foo {\n    fn extensions(&self) -> &[&str] { &[] }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_eq!(e.summary.as_deref(), Some("impl Extractor for Foo"));
    }

    #[test]
    fn summary_fallback_falls_through_to_the_first_symbol() {
        let src = "impl Wiki {\n    pub fn go(&self) {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_eq!(e.summary.as_deref(), Some("pub fn Wiki::go(&self)"));
    }

    #[test]
    fn summary_fallback_uses_a_generic_trait_impl_header() {
        // `impl<T: Clone> Display for Wrapper<T>` starts with `impl<`, not
        // `impl `, so a string sniff on the rendered signature misses it and
        // falls through to the qualified method — exactly the regression
        // this fallback exists to prevent.
        let src = "impl<T: Clone> Display for Wrapper<T> {\n    fn fmt(&self) {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_eq!(
            e.summary.as_deref(),
            Some("impl<T: Clone> Display for Wrapper<T>")
        );
    }

    #[test]
    fn summary_fallback_uses_an_unsafe_impl_header() {
        // `unsafe impl Send for Foo` starts with neither `impl ` nor `impl<`.
        // An empty impl body yields only the header symbol.
        let src = "unsafe impl Send for Foo {}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_eq!(e.symbols, vec!["unsafe impl Send for Foo".to_string()]);
        assert_eq!(e.summary.as_deref(), Some("unsafe impl Send for Foo"));
    }
}
