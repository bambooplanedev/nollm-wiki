//! Rust extraction: bare-`pub` visibility gating, owner qualification through
//! `impl`/`trait` scopes, and `#[cfg(test)]` module stripping.
use super::code::{
    default_shape, keep_all, no_export_set, sig_start_identity, LangSpec, Placement, Rank, Shape,
};
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator, Tree};

/// The `#[cfg(test)]` module scan's query, compiled once per process rather
/// than once per Rust file. See `code::QUERIES` for why.
static MOD_ITEM_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language: Language = tree_sitter_rust::LANGUAGE.into();
    Query::new(&language, "(mod_item) @m").expect("mod_item query must compile")
});

/// Force this module's query to compile. See `code::validate_queries`.
pub(crate) fn validate_queries() {
    LazyLock::force(&MOD_ITEM_QUERY);
}

/// Rust only. `(visibility_modifier)` also covers `pub(crate)`, `pub(super)`,
/// and `pub(in path)`, none of which leave the crate; bare `pub` is the only
/// one that belongs in `## Exports`.
fn keep_bare_pub(vis: &str) -> bool {
    vis == "pub"
}

/// The owner of a Rust definition, rendered as it will appear in the
/// signature:
///
///   * a field's or a variant's owner — the declaring type's name;
///   * inherent `impl` — the type name with generic arguments stripped, so
///     `impl<T> Holder<T>` yields `Holder::get`, valid Rust that sorts beside
///     the type's other methods;
///   * trait `impl` — see `trait_impl_owner`;
///   * `trait` declaration — the trait's own name.
fn rust_owner(def: Node, text: &str) -> Option<String> {
    let parent = def.parent()?;
    // The three list nodes that can hold a *named* member: `impl`/`trait`
    // bodies, struct/union field lists, and enum variant lists. A tuple
    // struct's `ordered_field_declaration_list` is deliberately absent — its
    // fields are positional and have no name to qualify.
    if !matches!(
        parent.kind(),
        "declaration_list" | "field_declaration_list" | "enum_variant_list"
    ) {
        return None;
    }
    let holder = parent.parent()?;
    match holder.kind() {
        // A field's or a variant's owner is just the declaring type's name.
        // `enum_variant` is not listed: a struct variant's fields are
        // qualifiable only through both the enum and the variant, and the
        // query never captures them (it requires the field list to be a
        // `struct_item`/`union_item` body) — this arm would be unreachable.
        // A trait's owner is likewise the trait's own name.
        "struct_item" | "union_item" | "enum_item" | "trait_item" => text
            .get(holder.child_by_field_name("name")?.byte_range())
            .map(str::to_string),
        // An inherent impl: the type name with generic arguments stripped,
        // so `impl<T> Holder<T>` yields `Holder::get` — valid Rust that
        // sorts beside the type's other methods.
        "impl_item" => trait_impl_owner(holder, text).or_else(|| {
            let ty = text.get(holder.child_by_field_name("type")?.byte_range())?;
            Some(ty.split('<').next().unwrap_or(ty).trim().to_string())
        }),
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
            "source_file"
            | "mod_item"
            | "declaration_list"
            | "impl_item"
            | "trait_item"
            | "struct_item"
            | "union_item"
            | "enum_item"
            | "field_declaration_list"
            | "enum_variant_list" => node = parent,
            _ => return false,
        }
    }
    true
}

/// The contiguous run of `attribute_item`s directly above `node`, nearest
/// first. Both callers need the whole run rather than just the nearest
/// attribute: `#[macro_export]` may sit above `#[doc(hidden)]`, and
/// `strip_rust_test_modules` splices from the run's outermost start so that
/// `#[cfg(test)]` itself is removed along with the module.
fn attribute_run(node: Node) -> Vec<Node> {
    let mut run = Vec::new();
    let mut prev = node.prev_named_sibling();
    while let Some(p) = prev {
        if p.kind() != "attribute_item" {
            break;
        }
        run.push(p);
        prev = p.prev_named_sibling();
    }
    run
}

/// True when the attribute run above `node` contains `want`, compared with
/// all whitespace removed so `#[cfg( test )]` matches `#[cfg(test)]`.
fn has_attribute(node: Node, text: &str, want: &str) -> bool {
    attribute_run(node).iter().any(|p| {
        text.get(p.byte_range())
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_whitespace())
            .eq(want.chars())
    })
}

/// The previous cycle's `rust_module_level` guard and `rust_owner` resolution,
/// composed. Order is unobservable: both were pure functions, and the old loop
/// called `rust_owner` only after `rust_module_level` had passed.
pub(crate) fn rust_placement(def: Node, text: &str) -> Placement {
    // A macro carries no visibility modifier, so the query cannot gate it
    // structurally the way it gates every other item: `#[macro_export]` is
    // the whole of a `macro_rules!` macro's public surface.
    if def.kind() == "macro_definition" && !has_attribute(def, text, "#[macro_export]") {
        return Placement::Rejected;
    }
    if !rust_module_level(def) {
        return Placement::Rejected;
    }
    match rust_owner(def, text) {
        Some(owner) => Placement::Scoped(vec![owner]),
        None => Placement::Scoped(Vec::new()),
    }
}

/// A trait impl rendered as an owner, in Rust's own disambiguation syntax:
/// `<Type as Trait>`. `None` when `impl_item` is an inherent impl.
///
/// The trait *must* be part of the owner: `impl Display for Foo` and
/// `impl Debug for Foo` both define `fn fmt(&self, …) -> Result`, which would
/// collapse to a single line under `dedup` if only the type were named. Keeping
/// the full type text also holds `impl Encode for Vec<u8>` and `… for Vec<u16>`
/// apart, and renders exotic targets as valid paths (`<&Foo as Trait>::m`).
///
/// Shared by both callers so they cannot drift: `rust_owner` reaches an
/// `impl_item` from a member inside its `declaration_list`, while
/// `rust_header_group` starts at the header itself — and a member must land in
/// the same group its own header does.
fn trait_impl_owner(impl_item: Node, text: &str) -> Option<String> {
    let ty = text.get(impl_item.child_by_field_name("type")?.byte_range())?;
    let tr = impl_item.child_by_field_name("trait")?;
    Some(format!("<{} as {}>", ty, text.get(tr.byte_range())?))
}

/// Whether an owner string rendered by `trait_impl_owner` names a trait
/// impl. Owner forms: `<Type as Trait>` for trait impls, the bare type for
/// inherent impls, the bare trait name for declarations, `Class.Inner` for
/// Python — only trait impls start with `<`. The one place that contract is
/// read; keep it next to the one place it is written.
pub(crate) fn is_trait_impl(owner: &str) -> bool {
    owner.starts_with('<')
}

/// The group a trait-impl header shares with its own methods.
pub(crate) fn rust_header_group(def: Node, text: &str) -> Option<String> {
    if def.kind() != "impl_item" {
        return None;
    }
    trait_impl_owner(def, text)
}

/// Rust's grammar-specific shapes. Everything here was previously a
/// `def.kind()` arm inside the shared core.
pub(crate) fn rust_shape(def: Node) -> Shape {
    match def.kind() {
        // A variant IS its payload. Cutting at `body` the way a struct is cut
        // would reduce `Remote(String)` to `Remote` and `Sqlite { path: String }`
        // to `Sqlite`, dropping the only part that says what the variant carries.
        // Variants are a single line's worth of text, so they render whole.
        //
        // A variant's discriminant rides along in that uncut span, so unlike
        // every other value it is unbudgeted. Recorded as a deferral, not fixed
        // here.
        "enum_variant" => Shape {
            rank: Rank::Def,
            cut: None,
            value: None,
            has_type: true,
        },
        // A `macro_definition`'s rules are plain children, not a `body` field,
        // so without an explicit cut the entire macro would become its
        // signature. The first rule opens right after the delimiter, leaving a
        // trailing `{` for `strip_trailing` to remove.
        "macro_definition" => {
            let mut cursor = def.walk();
            // Bound before the struct literal: the child iterator borrows
            // `cursor`, and inside the literal that borrow would outlive it.
            let cut = def.children(&mut cursor).find(|c| c.kind() == "macro_rule");
            Shape {
                rank: Rank::Def,
                cut,
                value: None,
                has_type: true,
            }
        }
        // A `const`/`static` cuts at its value and then re-appends it: the
        // contract is the type AND the value, and the value is the half a
        // reader cannot guess from the name. Always typed, so an over-budget
        // value is omitted rather than truncated.
        "const_item" | "static_item" => Shape {
            rank: Rank::Value,
            cut: def.child_by_field_name("value"),
            value: def.child_by_field_name("value"),
            has_type: true,
        },
        // A type alias is a value binding for ranking purposes, but it names
        // its target in a `type` field, not `value` — so `default_cut` finds
        // nothing, it renders whole, and it re-appends nothing. This is the
        // pairing that breaks if `value` is ever derived from `rank`.
        "type_item" => Shape {
            rank: Rank::Value,
            ..default_shape(def)
        },
        "mod_item" => Shape {
            rank: Rank::Module,
            ..default_shape(def)
        },
        _ => default_shape(def),
    }
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
        query_src: r"
                (function_item (visibility_modifier) @vis name: (identifier) @name) @def
                (struct_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (enum_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (trait_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (const_item (visibility_modifier) @vis name: (identifier) @name) @def
                (static_item (visibility_modifier) @vis name: (identifier) @name) @def
                (type_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (union_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (mod_item (visibility_modifier) @vis name: (identifier) @name) @def
                (macro_definition name: (identifier) @name) @def
                (struct_item (visibility_modifier) body: (field_declaration_list (field_declaration (visibility_modifier) @vis name: (field_identifier) @name) @def))
                (union_item (visibility_modifier) body: (field_declaration_list (field_declaration (visibility_modifier) @vis name: (field_identifier) @name) @def))
                (enum_item (visibility_modifier) @vis body: (enum_variant_list (enum_variant name: (identifier) @name) @def))
                (trait_item (visibility_modifier) @vis body: (declaration_list (associated_type name: (type_identifier) @name) @def))
                (trait_item (visibility_modifier) @vis body: (declaration_list (const_item name: (identifier) @name) @def))
                (impl_item trait: (_) type: (_)) @def
                (impl_item trait: (_) body: (declaration_list (function_item name: (identifier) @name) @def))
                (impl_item trait: (_) body: (declaration_list (type_item name: (type_identifier) @name) @def))
                (impl_item trait: (_) body: (declaration_list (const_item name: (identifier) @name) @def))
                (trait_item (visibility_modifier) @vis body: (declaration_list (function_signature_item name: (identifier) @name) @def))
                (trait_item (visibility_modifier) @vis body: (declaration_list (function_item name: (identifier) @name) @def))
                (use_declaration argument: (_) @import)
            ",
        name_filter: keep_all,
        vis_filter: keep_bare_pub,
        strip_trailing: &[';', '=', '{'],
        placement: rust_placement,
        owner_sep: "::",
        export_set: no_export_set,
        join_continuations: false,
        sig_start: sig_start_identity,
        header_group: rust_header_group,
    }
}

/// The outcome of a test-module scan.
///
/// `Unchanged` carries the tree the scan already parsed. Nothing was spliced,
/// so that tree still describes the caller's text exactly and extraction can
/// reuse it instead of parsing the same bytes a second time. Measured on 171
/// real crate files: 131 (77%) have no `#[cfg(test)]` module and reach this
/// arm. The other 23% must reparse — splicing changes the text, and no tree
/// survives an edit to its own source.
pub(crate) enum Stripped {
    Unchanged(Tree),
    Rewritten(String),
}

/// Remove `#[cfg(test)]`-annotated `mod` items from Rust source, replacing
/// each with a one-line omission marker (`// [tests omitted: mod <name>,
/// <N> lines]`). The spliced span starts at the first attribute in the
/// contiguous run of attributes directly above the `mod` (so `#[cfg(test)]`
/// itself is removed) and ends at the module's closing brace; `<N>` is that
/// span's line count. Returns `None` when there is nothing to strip or the
/// source fails to parse — the caller keeps the raw text.
pub(crate) fn strip_rust_test_modules(text: &str) -> Option<Stripped> {
    let language: Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(text, None)?;
    let query = &*MOD_ITEM_QUERY;
    let mut cursor = QueryCursor::new();

    // (start, end, mod name) per cfg(test) module, at any nesting depth.
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let node = cap.node;
            // The whole attribute run above the mod is spliced when any of
            // them is cfg(test), so `#[cfg(test)]` itself goes with it.
            if !has_attribute(node, text, "#[cfg(test)]") {
                continue;
            }
            let start = attribute_run(node)
                .last()
                .map_or_else(|| node.start_byte(), tree_sitter::Node::start_byte);
            let name = node
                .child_by_field_name("name")
                .and_then(|n| text.get(n.byte_range()))
                .unwrap_or("?")
                .to_string();
            spans.push((start, node.end_byte(), name));
        }
    }
    if spans.is_empty() {
        // Nothing spliced: hand back the tree so extraction need not reparse
        // bytes that have not changed.
        return Some(Stripped::Unchanged(tree));
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
    Some(Stripped::Rewritten(out))
}

#[cfg(test)]
mod tests {
    use crate::formats::code::testutil::{assert_has, assert_lacks};
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
        assert_has(&e.symbols, "pub fn render_page(x: i32) -> String");
        assert!(!e.symbols.iter().any(|s| s.contains("private_helper")));
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert_eq!(e.summary.as_deref(), Some("Module docs."));
    }

    #[test]
    fn a_const_only_module_without_docs_prefers_an_impl_header_to_the_const() {
        let src = "pub const LIMIT: u32 = 5;\nstruct Foo;\nimpl Display for Foo {\n    fn fmt(&self) {}\n}\n";
        let e = CodeExtractor.extract("cache.rs", src);
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
        let e = CodeExtractor.extract("c.rs", src);
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
        let e = CodeExtractor.extract("t.rs", src);
        assert_has(&e.symbols, "pub fn public_one()");
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
        assert_has(&e.symbols, "pub trait Public");
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
        let e = CodeExtractor.extract("query.rs", src);
        assert_has(
            &e.symbols,
            "pub fn Wiki::search(&self, q: &str) -> Vec<Hit>",
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
        assert_has(&e.symbols, "pub fn Holder::get(&self) -> T");
    }

    #[test]
    fn function_local_items_are_not_module_exports() {
        let src = "pub fn outer() {\n    pub struct Local;\n    impl Local {\n        pub fn hidden(&self) {}\n    }\n    pub fn nested() {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_has(&e.symbols, "pub fn outer()");
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
        let e = CodeExtractor.extract("t.rs", src);
        assert_has(&e.symbols, "pub fn in_mod()");
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
        let e = CodeExtractor.extract("text.rs", src);
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
        let e = CodeExtractor.extract("t.rs", src);
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
        let e = CodeExtractor.extract("t.rs", src);
        assert_has(&e.symbols, "fn <Vec<u8> as Encode>::go(&self) -> u8");
        assert_has(&e.symbols, "fn <Vec<u16> as Encode>::go(&self) -> u8");
    }

    #[test]
    fn exotic_impl_targets_render_as_valid_paths() {
        let src = "impl Trait for &Foo {\n    fn m(&self) {}\n}\nimpl Trait for (A, B) {\n    fn m(&self) {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_has(&e.symbols, "fn <&Foo as Trait>::m(&self)");
        assert_has(&e.symbols, "fn <(A, B) as Trait>::m(&self)");
    }

    #[test]
    fn associated_type_and_const_in_a_trait_impl_are_exported() {
        let src = "impl Iterator for Counter {\n    type Item = u32;\n    const FOO: u8 = 1;\n    fn next(&mut self) -> Option<u32> { None }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_has(&e.symbols, "type <Counter as Iterator>::Item = u32");
        assert_has(&e.symbols, "const <Counter as Iterator>::FOO: u8 = 1");
    }

    #[test]
    fn public_trait_declaration_lists_required_and_default_methods() {
        let src = "pub trait Extractor {\n    fn extensions(&self) -> &[&str];\n    fn helper(&self) -> u8 { 7 }\n}\ntrait Private {\n    fn n(&self);\n}\n";
        let e = CodeExtractor.extract("mod.rs", src);
        assert_has(&e.symbols, "fn Extractor::extensions(&self) -> &[&str]");
        assert_has(&e.symbols, "fn Extractor::helper(&self) -> u8");
        // Gated on the *trait's* visibility: a method inside a trait cannot
        // carry a modifier, but a private trait's methods are not exports.
        assert_lacks(&e.symbols, "Private");
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

    #[test]
    fn placement_reproduces_the_module_level_guard_and_owner() {
        let src = "pub struct Wiki;\nimpl Wiki {\n    pub fn search(&self) -> u8 { 0 }\n}\npub fn outer() {\n    pub fn hidden() {}\n}\n";
        let e = CodeExtractor.extract("q.rs", src);
        assert_has(&e.symbols, "pub fn Wiki::search(&self) -> u8");
        assert!(
            !e.symbols.iter().any(|s| s.contains("hidden")),
            "function-local item leaked: {:?}",
            e.symbols
        );
    }
}

#[cfg(test)]
mod surface_tests {
    use crate::formats::code::CodeExtractor;
    use crate::formats::Extractor;

    fn syms(src: &str) -> Vec<String> {
        CodeExtractor.extract("t.rs", src).symbols
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
        // The Python extractor has shown class fields since its own cycle;
        // a Rust struct's fields are the same contract and were invisible.
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
        let src = "pub enum Backend {\n    Memory,\n    Sqlite { path: String },\n    Remote(String),\n}\n";
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
        let e = CodeExtractor.extract("t.rs", src);
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
        // field inside it used to collapse into the signature. Measured in
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
        // `tidy_punctuation` used to be a flat substring pass over collapsed
        // text, so it rewrote punctuation *inside* a string literal that
        // survived into a signature: this rendered `", "`, silently
        // misreporting the constant's value. `render_span` now decides
        // structurally, leaving literal spans untidied.
        let src = "pub const SEP: &str = \" , \";\n";
        assert_has(src, "pub const SEP: &str = \" , \"");
    }
}
