//! Tests for the parent module.
use crate::formats::code::testutil::{assert_has, assert_lacks, item_lines};

#[test]
fn a_const_only_module_without_docs_prefers_an_impl_header_to_the_const() {
    let src =
        "pub const LIMIT: u32 = 5;\nstruct Foo;\nimpl Display for Foo {\n    fn fmt(&self) {}\n}\n";
    let e = crate::extract("cache.rs", src);
    assert_eq!(
        e.summary.as_deref(),
        Some("impl Display for Foo"),
        "FreeValue must rank below Header: {:?}",
        e.symbols
    );
}

#[test]
fn an_associated_const_in_a_trait_impl_is_a_member_not_a_free_value() {
    // Classification must run AFTER `placement`, so an associated item
    // inside a trait impl is a Member rather than a free value.
    //
    // Note what this test does NOT prove, despite its name: it cannot fail
    // by misclassification alone. `pick_summary_fallback` ranks Header
    // above FreeValue, so the impl header wins this summary either way.
    // Mutating the precedence in `classify` leaves the whole suite green —
    // see the note there for why the distinction is unobservable in
    // current output. What this pins is the summary itself.
    let src = "impl Iterator for Counter {\n    type Item = u32;\n    const FOO: u8 = 1;\n    fn next(&mut self) -> Option<u32> { None }\n}\n";
    let e = crate::extract("c.rs", src);
    assert_eq!(
        e.summary.as_deref(),
        Some("impl Iterator for Counter"),
        "symbols: {:?}",
        e.symbols
    );
}

#[test]
fn rust_restricted_visibility_is_not_an_export() {
    let src = "pub fn public_one() {}\npub(crate) fn crate_only() {}\npub(super) fn super_only() {}\npub(crate) struct CrateType;\nstruct Holder;\nimpl Holder {\n    pub fn kept(&self) {}\n    pub(crate) fn impl_crate_only(&self) {}\n}\n";
    let e = crate::extract("t.rs", src);
    assert_has(&e.symbols, "pub fn public_one()");
    assert!(
        e.symbols.iter().any(|s| s.contains("kept")),
        "a bare-pub inherent method must survive: {:?}",
        e.symbols
    );
    for leaked in ["crate_only", "super_only", "CrateType", "impl_crate_only"] {
        // each name is not exported outside the crate
        assert_lacks(&e.symbols, leaked);
    }
}

#[test]
fn rust_trait_visibility_gated() {
    let src = "pub trait Public {\n    fn m(&self);\n}\ntrait Private {\n    fn n(&self);\n}\n";
    let e = crate::extract("t.rs", src);
    assert_has(&e.symbols, "pub trait Public");
    // private trait leaked as export
    assert_lacks(&e.symbols, "Private");
}

#[test]
fn rust_module_level_const_static_and_type_alias() {
    let src = "pub const CACHE_VERSION: u32 = 1;\npub static NAME: &str = \"x\";\npub type Pack = Vec<u8>;\nconst PRIVATE: u8 = 3;\n";
    let e = crate::extract("t.rs", src);
    // A const's contract is its type AND its value; the value is the half a
    // reader cannot guess from the name.
    assert_has(&e.symbols, "pub const CACHE_VERSION: u32 = 1");
    assert_has(&e.symbols, "pub static NAME: &str = \"x\"");
    // An alias without its target would be useless, and `type_item` has no
    // `value:` field, so nothing is cut.
    assert_has(&e.symbols, "pub type Pack = Vec<u8>");
    assert_lacks(&e.symbols, "PRIVATE");
}

#[test]
fn inherent_impl_methods_are_qualified_with_their_type() {
    let src = "pub struct Wiki;\nimpl Wiki {\n    pub fn search(&self, q: &str) -> Vec<Hit> { todo!() }\n    fn helper(&self) {}\n}\npub fn free_function() {}\n";
    let e = crate::extract("query.rs", src);
    assert_has(
        &e.symbols,
        "pub fn Wiki::search(&self, q: &str) -> Vec<Hit>",
    );
    // a top-level function must stay unqualified
    assert_has(&e.symbols, "pub fn free_function()");
    assert_lacks(&e.symbols, "helper");
}

#[test]
fn generic_inherent_impl_strips_type_arguments() {
    let src = "impl<T: Clone> Holder<T> {\n    pub fn get(&self) -> T { todo!() }\n}\n";
    let e = crate::extract("t.rs", src);
    // `Holder::get` is valid Rust and sorts beside the type's other
    // methods; `Holder<T>::get` would be neither.
    assert_has(&e.symbols, "pub fn Holder::get(&self) -> T");
}

#[test]
fn function_local_items_are_not_module_exports() {
    let src = "pub fn outer() {\n    pub struct Local;\n    impl Local {\n        pub fn hidden(&self) {}\n    }\n    pub fn nested() {}\n}\n";
    let e = crate::extract("t.rs", src);
    assert_has(&e.symbols, "pub fn outer()");
    for leaked in ["Local", "hidden", "nested"] {
        // each name lives in a function body and is not module surface
        assert_lacks(&e.symbols, leaked);
    }
}

#[test]
fn function_local_trait_impls_and_traits_are_not_exports() {
    let src = "pub fn outer() {\n    struct Local;\n    impl Display for Local {\n        fn fmt(&self) {}\n    }\n    pub trait Hidden {\n        fn h(&self);\n    }\n}\n";
    let e = crate::extract("t.rs", src);
    assert_eq!(e.symbols, vec!["pub fn outer()".to_string()]);
}

#[test]
fn generic_trait_impl_keeps_its_type_arguments() {
    let src = "impl<T: Clone> Display for Wrapper<T> {\n    fn fmt(&self) {}\n}\n";
    let e = crate::extract("t.rs", src);
    assert_has(&e.symbols, "impl<T: Clone> Display for Wrapper<T>");
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
    let e = crate::extract("t.rs", src);
    assert_has(&e.symbols, "pub fn in_mod()");
}

#[test]
fn rust_test_module_stripped_with_marker() {
    let src = "//! Docs.\npub fn real() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn checks_real() {\n        real();\n    }\n}\n";
    let e = crate::extract("src/thing.rs", src);
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
    let e = crate::extract("t.rs", src);
    assert_eq!(e.body, src);
}

#[test]
fn rust_two_test_modules_get_two_markers_in_source_order() {
    let src = "pub fn a() {}\n\n#[cfg(test)]\nmod early_tests {\n    #[test]\n    fn t1() {}\n}\n\npub fn b() {}\n\n#[cfg(test)]\nmod late_tests {\n    #[test]\n    fn t2() {}\n}\n";
    let e = crate::extract("t.rs", src);
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
    let e = crate::extract("t.rs", src);
    assert_eq!(e.body.trim(), "// [tests omitted: mod tests, 5 lines]");
}

#[test]
fn code_after_a_test_module_on_the_same_line_is_not_commented_out() {
    let src = "pub fn before() {}\n#[cfg(test)]\nmod tests { #[test] fn t() {} } pub fn after() -> u8 { 1 }\n";
    let e = crate::extract("t.rs", src);
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
    // export following the test module on the same line was lost
    assert_has(&e.symbols, "pub fn after() -> u8");
}

#[test]
fn cfg_test_matches_modulo_whitespace_but_other_cfgs_do_not() {
    // Interior whitespace in the attribute still counts as cfg(test).
    let ws = "#[cfg( test )]\nmod tests {}\npub fn x() {}\n";
    let e = crate::extract("a.rs", ws);
    assert!(
        e.body.contains("// [tests omitted: mod tests, 2 lines]"),
        "body: {}",
        e.body
    );

    // A different cfg is NOT a test module.
    let feature = "#[cfg(feature = \"extra\")]\nmod extra {}\npub fn x() {}\n";
    let e2 = crate::extract("b.rs", feature);
    assert_eq!(e2.body, feature);
}

#[test]
fn symbols_and_imports_come_from_the_test_stripped_source() {
    let src = "pub struct Fixture;\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    use tempfile::tempdir;\n\n    impl Default for Fixture {\n        fn default() -> Self { Fixture }\n    }\n\n    #[test]\n    fn t() {\n        let _ = tempdir();\n    }\n}\n";
    let e = crate::extract("t.rs", src);
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
    // real export lost
    assert_has(&e.symbols, "pub struct Fixture");
    // The fixture also plants an `impl Default for Fixture` inside the
    // `#[cfg(test)]` module — exactly the widening the module-level
    // guard exists to prevent. It must not survive stripping.
    // impl inside the stripped test module leaked as an export
    assert_lacks(&e.symbols, "Default");
}

#[test]
fn trait_impl_emits_header_and_qualified_methods() {
    let src = "pub struct TextExtractor;\nimpl Extractor for TextExtractor {\n    fn extensions(&self) -> &[&str] { &[] }\n    fn extract(&self, p: &str) -> Entity { todo!() }\n}\n";
    let e = crate::extract("text.rs", src);
    assert_has(&e.symbols, "impl Extractor for TextExtractor");
    // Methods in a trait impl carry no visibility modifier — they are
    // public through the trait — so they must not be visibility-gated.
    assert_has(
        &e.symbols,
        "fn <TextExtractor as Extractor>::extract(&self, p: &str) -> Entity",
    );
}

#[test]
fn a_trait_impl_header_leads_its_own_methods() {
    let src = "pub struct TextExtractor;\nimpl Extractor for TextExtractor {\n    fn extensions(&self) -> &[&str] { &[] }\n    fn extract(&self) -> u8 { 0 }\n}\n";
    let e = crate::extract("text.rs", src);
    assert_eq!(
        e.symbols,
        vec![
            "impl Extractor for TextExtractor".to_string(),
            "fn <TextExtractor as Extractor>::extensions(&self) -> &[&str]".to_string(),
            "fn <TextExtractor as Extractor>::extract(&self) -> u8".to_string(),
            "pub struct TextExtractor".to_string(),
        ],
        "the header must lead its methods: {:?}",
        e.symbols
    );
}

#[test]
fn two_traits_with_the_same_method_name_stay_distinct() {
    let src = "pub struct Foo;\nimpl Display for Foo {\n    fn fmt(&self) -> Result { todo!() }\n}\nimpl Debug for Foo {\n    fn fmt(&self) -> Result { todo!() }\n}\n";
    let e = crate::extract("t.rs", src);
    assert_has(&e.symbols, "fn <Foo as Display>::fmt(&self) -> Result");
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
    let e = crate::extract("t.rs", src);
    assert_has(&e.symbols, "fn <Vec<u8> as Encode>::go(&self) -> u8");
    assert_has(&e.symbols, "fn <Vec<u16> as Encode>::go(&self) -> u8");
}

#[test]
fn exotic_impl_targets_render_as_valid_paths() {
    let src = "impl Trait for &Foo {\n    fn m(&self) {}\n}\nimpl Trait for (A, B) {\n    fn m(&self) {}\n}\n";
    let e = crate::extract("t.rs", src);
    assert_has(&e.symbols, "fn <&Foo as Trait>::m(&self)");
    assert_has(&e.symbols, "fn <(A, B) as Trait>::m(&self)");
}

#[test]
fn associated_type_and_const_in_a_trait_impl_are_exported() {
    let src = "impl Iterator for Counter {\n    type Item = u32;\n    const FOO: u8 = 1;\n    fn next(&mut self) -> Option<u32> { None }\n}\n";
    let e = crate::extract("t.rs", src);
    assert_has(&e.symbols, "type <Counter as Iterator>::Item = u32");
    assert_has(&e.symbols, "const <Counter as Iterator>::FOO: u8 = 1");
}

#[test]
fn public_trait_declaration_lists_required_and_default_methods() {
    let src = "pub trait Extractor {\n    fn extensions(&self) -> &[&str];\n    fn helper(&self) -> u8 { 7 }\n}\ntrait Private {\n    fn n(&self);\n}\n";
    let e = crate::extract("mod.rs", src);
    assert_has(&e.symbols, "fn Extractor::extensions(&self) -> &[&str]");
    assert_has(&e.symbols, "fn Extractor::helper(&self) -> u8");
    // Gated on the *trait's* visibility: a method inside a trait cannot
    // carry a modifier, but a private trait's methods are not exports.
    assert_lacks(&e.symbols, "Private");
}

#[test]
fn qualification_introduces_no_duplicate_symbols() {
    let src = "pub struct Wiki;\nimpl Wiki {\n    pub fn go(&self) {}\n}\nimpl Display for Wiki {\n    fn fmt(&self) {}\n}\npub fn go() {}\n";
    let e = crate::extract("t.rs", src);
    // Asserting the exact list, not `dedup().len()`: extraction already
    // deduplicates, so a self-comparison would pass no matter what. An
    // inherent method is matched by the top-level `function_item` pattern
    // and qualified by the ancestor walk — never by a second pattern,
    // which would emit it twice and let `dedup` hide the bug.
    assert_eq!(
        e.symbols,
        vec![
            "impl Display for Wiki".to_string(),
            "fn <Wiki as Display>::fmt(&self)".to_string(),
            "pub struct Wiki".to_string(),
            "pub fn Wiki::go(&self)".to_string(),
            "pub fn go()".to_string(),
        ]
    );
}

#[test]
fn summary_fallback_prefers_a_free_item_over_a_qualified_method() {
    let src = "pub struct TextExtractor;\nimpl Extractor for TextExtractor {\n    fn extensions(&self) -> &[&str] { &[] }\n}\n";
    let e = crate::extract("text.rs", src);
    assert_eq!(e.summary.as_deref(), Some("pub struct TextExtractor"));
}

#[test]
fn a_module_doc_sentence_wrapped_across_lines_summarizes_whole() {
    let src = "//! Rust extraction: bare-`pub` gating, owner qualification through\n//! impl scopes. Second sentence.\n//!\n//! Next paragraph.\n\npub fn f() {}\n";
    let e = crate::extract("t.rs", src);
    assert_eq!(
        e.summary.as_deref(),
        Some("Rust extraction: bare-`pub` gating, owner qualification through impl scopes.")
    );
}

#[test]
fn summary_fallback_uses_the_impl_header_when_no_free_item_exists() {
    let src = "impl Extractor for Foo {\n    fn extensions(&self) -> &[&str] { &[] }\n}\n";
    let e = crate::extract("t.rs", src);
    assert_eq!(e.summary.as_deref(), Some("impl Extractor for Foo"));
}

#[test]
fn summary_fallback_falls_through_to_the_first_symbol() {
    let src = "impl Wiki {\n    pub fn go(&self) {}\n}\n";
    let e = crate::extract("t.rs", src);
    assert_eq!(e.summary.as_deref(), Some("pub fn Wiki::go(&self)"));
}

#[test]
fn summary_fallback_uses_a_generic_trait_impl_header() {
    // `impl<T: Clone> Display for Wrapper<T>` starts with `impl<`, not
    // `impl `, so a string sniff on the rendered signature misses it and
    // falls through to the qualified method — exactly the regression
    // this fallback exists to prevent.
    let src = "impl<T: Clone> Display for Wrapper<T> {\n    fn fmt(&self) {}\n}\n";
    let e = crate::extract("t.rs", src);
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
    let e = crate::extract("t.rs", src);
    assert_eq!(e.symbols, vec!["unsafe impl Send for Foo".to_string()]);
    assert_eq!(e.summary.as_deref(), Some("unsafe impl Send for Foo"));
}

#[test]
fn placement_reproduces_the_module_level_guard_and_owner() {
    let src = "pub struct Wiki;\nimpl Wiki {\n    pub fn search(&self) -> u8 { 0 }\n}\npub fn outer() {\n    pub fn hidden() {}\n}\n";
    let e = crate::extract("q.rs", src);
    assert_has(&e.symbols, "pub fn Wiki::search(&self) -> u8");
    // function-local item leaked
    assert_lacks(&e.symbols, "hidden");
}

#[test]
fn items_include_private_fns_and_methods_with_doc_and_attribute_lines() {
    let src = "/// Adds one.\n#[inline]\npub fn add(x: u8) -> u8 {\n    x + 1\n}\n\nfn helper() {}\n\npub struct S;\nimpl S {\n    /// Builds it.\n    fn new() -> S {\n        S\n    }\n}\n";
    let e = crate::extract("m.rs", src);
    assert_eq!(
        item_lines(&e.items),
        vec![
            "1-5 fn add pub",
            "7-7 fn helper priv",
            "9-9 type S pub",
            "11-14 method S::new priv",
        ]
    );
    let add = &e.items[0];
    assert_eq!(add.signature, "pub fn add(x: u8) -> u8");
    assert_eq!(add.doc.as_deref(), Some("Adds one."));
    assert_eq!(e.items[3].signature, "fn new() -> S");
    assert_eq!(e.items[3].doc.as_deref(), Some("Builds it."));
    // exports untouched: still public-only
    assert_has(&e.symbols, "pub fn add(x: u8) -> u8");
    assert_lacks(&e.symbols, "helper");
    assert_lacks(&e.symbols, "new");
}

#[test]
fn two_leads_on_one_line_keep_the_doc_above() {
    let src = "/// Packed.\n#[derive(Debug)] #[repr(C)]\npub struct P;\n";
    let e = crate::extract("m.rs", src);
    assert_eq!(item_lines(&e.items), vec!["1-3 type P pub"]);
    assert_eq!(e.items[0].doc.as_deref(), Some("Packed."));
}

#[test]
fn items_after_a_mid_file_test_module_keep_their_original_lines() {
    let src = "pub fn a() {}\n#[cfg(test)]\nmod tests {\n    pub fn helper() {}\n    #[test]\n    fn t() {}\n}\npub fn b() {}\n";
    let e = crate::extract("m.rs", src);
    // `helper` is bare `pub`, but its module is stripped from `## Exports`,
    // so it must not read as public either.
    assert_eq!(
        item_lines(&e.items),
        vec![
            "1-1 fn a pub",
            "2-7 module tests priv",
            "4-4 fn helper priv",
            "5-6 fn t priv",
            "8-8 fn b pub",
        ]
    );
    assert_lacks(&e.symbols, "helper");
    assert!(e.body.contains("[tests omitted: mod tests"));
    assert_lacks(&e.symbols, "fn t");
}

#[test]
fn a_doc_comment_parted_by_a_blank_line_or_plain_comment_is_not_attached() {
    let src =
        "/// Lost.\n\nfn a() {}\n/// Also lost.\n// plain\nfn b() {}\n//// Not doc.\nfn c() {}\n";
    let e = crate::extract("m.rs", src);
    assert_eq!(
        item_lines(&e.items),
        vec!["3-3 fn a priv", "6-6 fn b priv", "8-8 fn c priv"]
    );
    assert!(e.items.iter().all(|i| i.doc.is_none()));
}

#[test]
fn item_doc_is_the_first_sentence_joined_across_lines_and_capped() {
    let src = format!(
            "/// Splits the input into\n/// words. Then more.\nfn split() {{}}\n/// {}\nfn long() {{}}\n",
            "word ".repeat(60)
        );
    let e = crate::extract("m.rs", &src);
    assert_eq!(
        e.items[0].doc.as_deref(),
        Some("Splits the input into words.")
    );
    let long = e.items[1].doc.as_deref().unwrap();
    assert_eq!(long.chars().count(), 120);
    assert!(long.ends_with('…'));
}

#[test]
fn a_type_alias_is_a_type_item_like_in_every_other_language() {
    let src = "pub type Id = u64;\ntype Local = u8;\npub const N: u8 = 1;\n";
    let e = crate::extract("m.rs", src);
    assert_eq!(
        item_lines(&e.items),
        vec!["1-1 type Id pub", "2-2 type Local priv", "3-3 value N pub"]
    );
}

#[test]
fn the_existing_gate_decides_public_for_restricted_and_trait_impl_items() {
    let src = "pub(crate) fn c() {}\nstruct T;\nimpl Display for T {\n    fn fmt(&self) {}\n}\n";
    let e = crate::extract("m.rs", src);
    assert_eq!(
        item_lines(&e.items),
        vec![
            "1-1 fn c priv",
            "2-2 type T priv",
            "4-4 method <T as Display>::fmt pub",
        ]
    );
}
