//! Rust extraction: bare-`pub` visibility gating, owner qualification through
//! `impl`/`trait` scopes, and `#[cfg(test)]` module stripping.
use super::code::{default_shape, LangSpec, Lead, Placement, Rank, Shape};
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

/// The `rust_module_level` guard and `rust_owner` resolution, composed. Order
/// is unobservable: both are pure, and `rust_owner` runs only once the guard passes.
fn rust_placement(def: Node, text: &str) -> Placement {
    // A macro carries no visibility modifier, so the query cannot gate it
    // structurally the way it gates every other item: `#[macro_export]` is
    // the whole of a `macro_rules!` macro's public surface.
    if def.kind() == "macro_definition" && !has_attribute(def, text, "#[macro_export]") {
        return Placement::Rejected;
    }
    if !rust_module_level(def) {
        return Placement::Rejected;
    }
    Placement::Scoped(rust_owner(def, text).into_iter().collect())
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
fn rust_header_group(def: Node, text: &str) -> Option<String> {
    if def.kind() != "impl_item" {
        return None;
    }
    trait_impl_owner(def, text)
}

/// Rust's grammar-specific shapes.
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
            cut: None,
            ..default_shape(def)
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
                cut,
                ..default_shape(def)
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

// Requiring `(visibility_modifier)` excludes private items
// structurally; `vis_filter` then drops the restricted forms
// (`pub(crate)` and friends) that the node kind also covers.
// The trait-impl patterns deliberately capture no `@vis`: rustc
// rejects a visibility modifier there, and those items are public
// through the trait. The trait-declaration patterns gate on the
// trait's own visibility instead of the method's.
const QUERY: &str = r"
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
                (function_item name: (identifier) @name) @item
                (function_signature_item name: (identifier) @name) @item
                (const_item name: (identifier) @name) @item
                (static_item name: (identifier) @name) @item
                (mod_item name: (identifier) @name) @item
                (struct_item name: (type_identifier) @name) @item
                (enum_item name: (type_identifier) @name) @item
                (trait_item name: (type_identifier) @name) @item
                (union_item name: (type_identifier) @name) @item
                (type_item name: (type_identifier) @name) @item
            ";

pub(crate) fn rust_spec() -> LangSpec {
    LangSpec {
        vis_filter: keep_bare_pub,
        strip_trailing: &[';', '=', '{'],
        placement: rust_placement,
        owner_sep: "::",
        header_group: rust_header_group,
        item_lead: rust_lead,
        // Every public item also matches a @def pattern in `QUERY`, whose gate
        // decides it; an @item-only match is private or restricted.
        query_gates_visibility: true,
        shape: rust_shape,
        ..LangSpec::base("rust", tree_sitter_rust::LANGUAGE.into(), QUERY)
    }
}

/// A Rust item's leads: its attributes, and its `///` outer doc lines.
/// `////` is a plain comment by rustc's own rule, and `//!` documents the
/// enclosing module, not the next item, so both stop the walk.
fn rust_lead(node: Node, text: &str) -> Lead {
    match node.kind() {
        "attribute_item" => Lead::Attr,
        "line_comment" => match text
            .get(node.byte_range())
            .and_then(|t| t.strip_prefix("///"))
        {
            Some(doc) if !doc.starts_with('/') => Lead::Doc(doc.trim().to_string()),
            _ => Lead::Stop,
        },
        _ => Lead::Stop,
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
///
/// `Rewritten` carries the tree of the ORIGINAL text: items are extracted
/// from it, so their lines stay true to the file even past a stripped
/// module. It also carries the stripped modules' 1-based inclusive line
/// ranges in that original text.
pub(crate) enum Stripped {
    Unchanged(Tree),
    Rewritten(String, Tree, Vec<(usize, usize)>),
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
        for cap in m.captures() {
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
    let line_of = |byte: usize| text[..byte].matches('\n').count() + 1;
    let ranges = spans
        .iter()
        .map(|&(start, end, _)| (line_of(start), line_of(end)))
        .collect();

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
    Some(Stripped::Rewritten(out, tree, ranges))
}

/// Whether the attributes above the item starting at 1-based `start_line`
/// include `#[test]` or `#[tokio::test]`. Doc comments may sit between them.
pub(crate) fn has_test_attribute(text: &str, start_line: usize) -> bool {
    text.lines()
        .skip(start_line - 1)
        .map(str::trim_start)
        .take_while(|l| l.starts_with("#[") || l.starts_with("//"))
        .any(|l| {
            ["#[test]", "#[tokio::test]", "#[tokio::test("]
                .iter()
                .any(|a| l.starts_with(a))
        })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod surface_tests;
