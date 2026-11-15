//! Tests for `extract_rust`: which items the exported surface lists, and signature rendering.

fn syms(src: &str) -> Vec<String> {
    crate::extract("t.rs", src).symbols
}

/// These curry `syms` onto the shared assertions; the matching rules and
/// the failure messages live in `code::testutil`.
fn assert_has(src: &str, want: &str) {
    crate::formats::code::testutil::assert_has(&syms(src), want);
}

fn assert_lacks(src: &str, unwanted: &str) {
    crate::formats::code::testutil::assert_lacks(&syms(src), unwanted);
}

#[test]
fn a_public_union_and_its_fields_are_exported() {
    let src =
        "pub union Raw {\n    pub bits: u64,\n    pub halves: [u32; 2],\n    private: u8,\n}\n";
    assert_has(src, "pub union Raw");
    assert_has(src, "pub Raw::bits: u64");
    assert_has(src, "pub Raw::halves: [u32; 2]");
    assert_lacks(src, "private");
}

#[test]
fn public_struct_fields_are_exported_and_private_ones_are_not() {
    // Matches the Python extractor's class fields: a Rust struct's public
    // fields are part of the same contract.
    let src = "pub struct Store {\n    pub path: String,\n    pub(crate) internal: u8,\n    seen: u32,\n}\n";
    assert_has(src, "pub struct Store");
    assert_has(src, "pub Store::path: String");
    assert_lacks(src, "internal"); // pub(crate) does not leave the crate
    assert_lacks(src, "seen");
}

#[test]
fn enum_variants_keep_their_payload() {
    // Cutting a variant at its `body` the way a struct is cut would
    // reduce `Remote(String)` to `Remote`, dropping the only part that
    // says what the variant carries.
    let src =
        "pub enum Backend {\n    Memory,\n    Sqlite { path: String },\n    Remote(String),\n}\n";
    assert_has(src, "pub enum Backend");
    assert_has(src, "Backend::Memory");
    assert_has(src, "Backend::Sqlite { path: String }");
    assert_has(src, "Backend::Remote(String)");
}

#[test]
fn a_struct_variants_fields_are_not_exported_on_their_own() {
    // `path` here is reachable only through `Backend::Sqlite`. Emitting it
    // as a member would put a bare, unqualifiable `pub path: String` in
    // `## Exports`; the variant already renders its fields verbatim.
    let src = "pub enum Backend {\n    Sqlite { pub path: String },\n}\n";
    assert_lacks(src, "::path");
}

#[test]
fn a_public_module_declaration_is_exported() {
    assert_has("pub mod backends;\n", "pub mod backends");
    assert_has("pub mod inline {\n    pub fn f() {}\n}\n", "pub mod inline");
    assert_lacks("mod private_mod;\n", "private_mod");
    assert_lacks("pub(crate) mod internal;\n", "internal");
}

#[test]
fn a_traits_associated_items_are_exported_like_its_methods() {
    // The impl side of both already rendered (`type <Store as Persist>::Key`),
    // so their absence on the declaration side was an asymmetry visible
    // within a single page.
    let src =
        "pub trait Persist {\n    type Key;\n    const VERSION: u32;\n    fn save(&self);\n}\n";
    assert_has(src, "type Persist::Key");
    assert_has(src, "const Persist::VERSION: u32");
    assert_has(src, "fn Persist::save(&self)");
}

#[test]
fn a_private_traits_associated_items_stay_private() {
    let src = "trait Hidden {\n    type Key;\n    const VERSION: u32;\n}\n";
    assert_lacks(src, "Key");
    assert_lacks(src, "VERSION");
}

#[test]
fn only_an_exported_macro_is_a_symbol() {
    let exported = "#[macro_export]\nmacro_rules! store_key {\n    ($a:expr) => { $a };\n}\n";
    assert_has(exported, "macro_rules! store_key");
    // The rules are the body, not the signature — none of it may leak in.
    assert_lacks(exported, "expr");

    let internal = "macro_rules! helper {\n    () => { 1 };\n}\n";
    assert_lacks(internal, "helper");
}

#[test]
fn an_exported_macro_keeps_its_other_attributes() {
    // The attribute run is contiguous; `#[macro_export]` need not be the
    // one directly above the macro.
    let src = "#[macro_export]\n#[doc(hidden)]\nmacro_rules! k {\n    () => { 1 };\n}\n";
    assert_has(src, "macro_rules! k");
}

#[test]
fn items_inside_a_function_body_are_still_rejected() {
    // Widening `rust_module_level` for fields and variants must not open
    // a path out of a function body.
    let src = "pub fn f() {\n    pub struct Inner { pub x: u8 }\n    pub enum E { A }\n}\n";
    assert_lacks(src, "Inner");
    assert_lacks(src, "::x");
    assert_lacks(src, "E::A");
}

#[test]
fn a_module_never_takes_the_summary_from_a_real_definition() {
    // `pub mod a` sorts below `pub struct Store` as a bare signature, so
    // without an explicit rank a module list would take over the summary
    // of every module that declares one (`lib.rs` declares nothing else).
    let src = "pub mod a;\npub struct Store;\n";
    let e = crate::extract("t.rs", src);
    assert_eq!(e.summary.as_deref(), Some("pub struct Store"));
}

#[test]
fn an_oversized_rust_value_is_omitted_not_truncated() {
    // 48 chars of a tree-sitter query source or a long aggregate is a
    // fragment of grammar syntax — measurably worse than the type it would
    // replace. Rust always has a type to fall back on, so it omits.
    let long = "x".repeat(200);
    let src = format!("pub const QUERY: &str = \"{long}\";\n");
    assert_has(&src, "pub const QUERY: &str");
    let s = syms(&src);
    assert!(
        !s.iter().any(|x| x.contains('…')),
        "Rust must never truncate a value: {s:?}"
    );
}

#[test]
fn a_valueless_associated_const_renders_unchanged() {
    // A trait *declaration*'s const has no value node at all. It must not
    // pick up a dangling ` = `.
    let src = "pub trait Persist {\n    const VERSION: u32;\n}\n";
    assert_has(src, "const Persist::VERSION: u32");
    let s = syms(src);
    assert!(
        !s.iter().any(|x| x.contains("VERSION: u32 =")),
        "no dangling `=`: {s:?}"
    );
}

#[test]
fn an_enum_variant_does_not_render_its_inner_doc_comments() {
    // A variant renders its whole node, body included, so a `///` on a
    // field inside it would collapse into the signature. Measured in
    // real crates: aho-corasick renders `MatchErrorKind::UnsupportedStream
    // { /// The match semantics ... got: MatchKind, }`. Beyond being noise
    // that inflates token_estimate, a `///` inside a one-line signature
    // comments out everything after it for anyone who copies it.
    let src = "pub enum E {\n    /// Doc on the variant.\n    V {\n        /// Doc on the field.\n        got: Kind,\n    },\n}\n";
    assert_has(src, "E::V { got: Kind, }");
    assert_lacks(src, "///");
    assert_lacks(src, "Doc on");
}

#[test]
fn a_block_comment_inside_a_signature_span_is_dropped() {
    let src = "pub fn f(a: /* why */ u8) -> u8 { 0 }\n";
    assert_has(src, "pub fn f(a: u8) -> u8");
}

#[test]
fn a_comment_is_dropped_without_welding_the_tokens_around_it() {
    // Dropping the comment's bytes must leave the whitespace on either
    // side intact, or `pub /*c*/ fn f` renders `pubfn f`.
    let src = "pub /* c */ fn f() {}\n";
    assert_has(src, "pub fn f()");
}

#[test]
fn a_rust_string_value_keeps_its_own_punctuation() {
    // A flat `tidy_punctuation` pass over collapsed text would rewrite
    // punctuation *inside* a string literal, rendering `", "` and silently
    // misreporting the constant's value. `render_span` decides
    // structurally, leaving literal spans untidied.
    let src = "pub const SEP: &str = \" , \";\n";
    assert_has(src, "pub const SEP: &str = \" , \"");
}
