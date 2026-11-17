//! C++: a language whose public surface is a header's declarations, with owner
//! resolution through class bodies and qualified names, and no visibility gate.
use super::code::{
    default_cut, default_shape, outermost, plain_comment_text, value_shape, LangSpec, Lead,
    Placement, Shape,
};
use crate::model::PairRole;
use tree_sitter::{Node, Parser, Tree};

mod pairs;

pub use pairs::pair_declarations;

/// Every capture shape a definition's name can take. The general `(_)` form
/// must NOT be used where a qualified-name pattern can also match:
/// tree-sitter emits one match per pattern with no specificity resolution, so
/// a general pattern alongside the qualified-name patterns below would yield
/// two entries for one definition — one with the owner spliced into an
/// already-qualified name — and `dedup_by` would not collapse them, because
/// their signatures differ.
///
/// The wrapper patterns require the `declarator:` field to be a
/// `pointer_declarator` or a `reference_declarator` around a
/// `function_declarator`, one level deep. The one on `function_definition`
/// mirrors the unwrapped ones: a plain name, or a qualified name of one or
/// two qualifier levels, so `Level* OrderBook::top()` captures `top` and
/// `qualified_owner` (which descends the same one wrapper) supplies
/// `OrderBook`, as for `void OrderBook::apply()`. A general `(_)` capture
/// there would match the qualified name too and splice the owner ahead of the
/// qualifier the source already wrote;
/// `a_pointer_returning_out_of_line_definition_renders_its_owner_once` pins
/// that. The two on `field_declaration`/`declaration` stay general `(_)`:
/// no pattern there resolves an owner through a qualifier, so there is
/// nothing to double.
///
/// Each wrapper set is one pattern with alternations, not one pattern per
/// shape: `Query::new` cost grows with the pattern count, and this query is
/// built once per process. The plain names inside a wrapper alternation are
/// listed flat, each with its own `@name`: a nested `[(identifier) …] @name`
/// inside the outer alternation silently drops the `field_identifier` match
/// of `const Stats& stats() const { … }` (tree-sitter 0.27).
///
/// `function_definition` alone covers only members with a BODY, so a
/// declaration-only header would carry nothing but `class OrderBook`;
/// `field_declaration` and `declaration` are what reach the rest, and are also
/// what admit constructors (an `identifier` declarator, because the grammar
/// cannot know the name is the class's), destructors and operators.
///
/// Types and functions are indexed; DATA is not — no member variables, no
/// file-scope globals or constants. The one exception is the `@item` pattern
/// at the end: a namespace- or file-scope variable is an item (kind `value`),
/// never an export. `field_declaration.declarator` is
/// `multiple`, so `int a, b, c;` emits three matches over one node, and
/// `default_cut` finds no `body` field. An item's signature is then the
/// declaration's specifiers plus that declarator's own text (`int a`, `int *b`,
/// `int f()`, `int *g()`; see `shared_declaration_signature`), while the
/// `## Exports` line still renders the whole declaration with the owner spliced
/// in. A struct's layout stays visible in the page's `## Body`, which renders
/// the source verbatim.
///
/// `Entity::methods` shares the node too: `extract_code`'s `is_function` check
/// reads `def.child_by_field_name("declarator")`, which, because that field is
/// `multiple`, always returns the FIRST declarator on the shared node. For
/// `int *g(), f();` that is a wrapper around a `function_declarator`, so both
/// matches classify as functions and both reach `methods`, which is the right
/// answer. It stays a latent mis-read (a declaration mixing a function and a
/// data declarator would classify both alike), pre-existing and rare:
/// documented, not fixed.
///
/// A conversion operator has two shapes. Parsed correctly, it is an
/// `operator_cast` declarator with no `function_declarator` around it, which
/// the `operator_cast` pattern below captures whole. After `inline` or
/// `constexpr`, `K::operator bool()` parses as a function `bool` returning
/// the type `K::operator`, which the plain patterns capture: `misread_cast`
/// tells it, and it is the method `K::operator bool`, its signature as
/// written. `code::name_text` cuts either one's name to `operator bool`.
///
/// KNOWINGLY UNCOVERED, each observed against the real grammar:
///   * declarator nesting beyond ONE wrapper — the wrapper patterns reach
///     exactly one deep, and `int** f()` is
///     `(pointer_declarator (pointer_declarator (function_declarator …)))`;
///   * qualifier nesting beyond two — `a::b::C::m` needs a third alternative;
///   * an out-of-line nested class (`class Outer::Inner { … }`), whose
///     `class_specifier` names itself with a `qualified_identifier`, not the
///     `type_identifier` these patterns require;
///   * class template specializations, whose head is a `template_declaration`
///     wrapping a `class_specifier` named by a `template_type`. A function's
///     explicit specialization (`template <> int blank<int>()`) is an item:
///     its declarator names a `template_function`, captured whole, so the
///     item is named `blank<int>` as written and stays apart from the
///     primary template `blank`;
///   * a file-scope most-vexing parse (`Level* p(q);` at namespace scope)
///     matches the pointer-declarator wrapper pattern and is exported as a
///     function: `cpp_placement`'s rejection only fires inside a
///     `compound_statement`, which a namespace or file scope is not.
const QUERY: &str = r"
(function_definition declarator: (function_declarator
  declarator: [(identifier) (field_identifier) (destructor_name) (operator_name) (template_function)] @name)) @def
(field_declaration declarator: (function_declarator
  declarator: [(identifier) (field_identifier) (destructor_name) (operator_name) (template_function)] @name)) @def
(declaration declarator: (function_declarator
  declarator: [(identifier) (field_identifier) (destructor_name) (operator_name) (template_function)] @name)) @def

(function_definition declarator: (function_declarator declarator: [
    (qualified_identifier name: [(identifier) (destructor_name) (operator_name) (template_function)] @name)
    (qualified_identifier name: (qualified_identifier name: [(identifier) (destructor_name) (operator_name) (template_function)] @name))])) @def

(function_definition declarator: [
  (pointer_declarator (function_declarator declarator: [
    (identifier) @name (field_identifier) @name (destructor_name) @name (operator_name) @name (template_function) @name
    (qualified_identifier name: [(identifier) (destructor_name) (operator_name) (template_function)] @name)
    (qualified_identifier name: (qualified_identifier name: [(identifier) (destructor_name) (operator_name) (template_function)] @name))]))
  (reference_declarator (function_declarator declarator: [
    (identifier) @name (field_identifier) @name (destructor_name) @name (operator_name) @name (template_function) @name
    (qualified_identifier name: [(identifier) (destructor_name) (operator_name) (template_function)] @name)
    (qualified_identifier name: (qualified_identifier name: [(identifier) (destructor_name) (operator_name) (template_function)] @name))]))]) @def
(field_declaration declarator: [
  (pointer_declarator (function_declarator declarator: (_) @name))
  (reference_declarator (function_declarator declarator: (_) @name))]) @def
(declaration declarator: [
  (pointer_declarator (function_declarator declarator: (_) @name))
  (reference_declarator (function_declarator declarator: (_) @name))]) @def

(function_definition declarator: [
  (operator_cast) @name
  (qualified_identifier name: (operator_cast) @name)
  (qualified_identifier name: (qualified_identifier name: (operator_cast) @name))]) @def
(declaration declarator: (operator_cast) @name) @def

(class_specifier   name: (type_identifier) @name) @def
(struct_specifier  name: (type_identifier) @name) @def
(union_specifier   name: (type_identifier) @name) @def
(enum_specifier    name: (type_identifier) @name) @def
(alias_declaration name: (type_identifier) @name) @def
(type_definition   declarator: (type_identifier) @name) @def

(declaration declarator: [
  (identifier) @name
  (pointer_declarator declarator: (identifier) @name)
  (reference_declarator (identifier) @name)
  (array_declarator declarator: (identifier) @name)
  (init_declarator declarator: [
    (identifier) @name
    (pointer_declarator declarator: (identifier) @name)
    (reference_declarator (identifier) @name)
    (array_declarator declarator: (identifier) @name)])]) @item

(preproc_include path: (_) @import)
";

pub(crate) fn cpp_spec() -> LangSpec {
    LangSpec {
        // `vis_filter` stays at the base default: everything visible is
        // extracted, private members included. Gating on
        // `access_specifier` would be inconsistent — an out-of-line
        // `void OrderBook::rebuild_()` carries no visible specifier and would
        // be kept anyway — and a sticky previous-sibling scan would default to
        // private and drop every member nested in a `template_declaration` or
        // a `preproc_ifdef`, neither of which has a sibling specifier in the
        // `field_declaration_list`.
        // `OrderBook() = default;` renders whole: `default_method_clause` is
        // not the `body` field, so no cut applies. `declaration` and
        // `field_declaration` also carry their `;`.
        strip_trailing: &[';'],
        placement: cpp_placement,
        owner_sep: "::",
        sig_start: cpp_sig_start,
        owner_anchor: cpp_owner_anchor,
        item_lead: cpp_lead,
        shape: cpp_shape,
        ..LangSpec::base("cpp", tree_sitter_cpp::LANGUAGE.into(), QUERY)
    }
}

/// One line of a C++ comment as doc text, tracking whether `in_block` is inside
/// a still-open Doxygen block (`/**` or `/*!` not yet closed by `*/`): the only
/// context where a bare `*` continuation is doc content, since a plain
/// `/* ... */` license block continues the same way. Shared by the file-summary
/// scan and by item docs, so there is one copy of the tracking.
pub(crate) fn cpp_doc_step(line: &str, in_block: &mut bool) -> Option<String> {
    let opens = line.starts_with("/**") || line.starts_with("/*!");
    let inside = *in_block || opens;
    let result = cpp_doc_line(line, inside);
    *in_block = inside && !line.contains("*/");
    result
}

/// True for a line that reads as a COMMENT, or as a blank line between
/// comments, even though `cpp_doc_line` rejected it as a doc marker — a
/// `// Copyright` banner, an SPDX line, or a plain `/*`/`*` continuation of a
/// non-Doxygen block comment. False for actual code.
///
/// This is what bounds the C++ banner-skip in `leading_doc` to an actual
/// banner: without it, the skip cannot tell a license header from
/// `#include <vector>`, `class OrderBook {`, or `public:`, and scans clean
/// through them looking for the first doc marker anywhere in the file —
/// which is exactly the whole-file-scan defect the C++-only gate exists to
/// avoid in the first place, just relocated one line down. The blank-line
/// allowance is what lets a banner followed by a blank separator line still
/// reach the doc comment beneath it, pinned by
/// `doc_comment_below_a_license_banner_is_still_found`.
pub(crate) fn is_banner_shaped(line: &str) -> bool {
    line.is_empty() || line.starts_with("//") || line.starts_with("/*") || line.starts_with('*')
}

/// One C++ doc line, or `None` when the line is not documentation.
///
/// Only real markers count — `///`, `//!`, `/**`, `/*!` — so a `// Copyright`
/// banner is not a doc comment. Punctuation-only results are rejected, which
/// is what keeps an LLVM `//===--- … ---===//` header and a `/*****/` rule out
/// of the summary. Continuation lines shed a leading `*`, a `@brief`/`\brief`
/// tag, and the closing `*/`.
///
/// The bare-`*` continuation form is only accepted while `in_doxygen_block`
/// is true — i.e. while the caller (`leading_doc`) has tracked us as still
/// inside a block that actually opened with `/**`/`/*!`. A `*`-prefixed line
/// is ALSO how a plain `/* Copyright ... */` block continues (`* All rights
/// reserved.`), and without this gate that banner text reads as a doc
/// continuation and gets concatenated onto whatever real `///` doc follows —
/// e.g. `"All rights reserved.\nMaintains the book."` — since this function
/// sees one line at a time and cannot otherwise tell which kind of block
/// opened it.
fn cpp_doc_line(line: &str, in_doxygen_block: bool) -> Option<String> {
    let body = line
        .strip_prefix("///")
        .or_else(|| line.strip_prefix("//!"))
        .or_else(|| line.strip_prefix("/**"))
        .or_else(|| line.strip_prefix("/*!"))
        .or_else(|| in_doxygen_block.then(|| line.strip_prefix('*')).flatten())?;
    let cleaned = body
        .trim()
        .trim_end_matches("*/")
        .trim()
        .trim_start_matches("@brief")
        .trim_start_matches("\\brief")
        .trim()
        .to_string();
    Some(if cleaned.chars().any(char::is_alphanumeric) {
        cleaned
    } else {
        String::new()
    })
}

/// The doc text of one C++ comment node, or `None` when it is not a Doxygen
/// comment or says nothing.
fn cpp_comment_doc(comment: &str) -> Option<String> {
    let mut in_block = false;
    let lines: Vec<String> = comment
        .lines()
        .map(str::trim)
        .filter_map(|l| cpp_doc_step(l, &mut in_block))
        .filter(|l| !l.is_empty())
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// A Doxygen comment directly above an item is its doc, and any other comment
/// a plain lead; anything else ends the walk. A member-trailing doc (`///<`,
/// `//!<`, `/**<`, `/*!<`) documents the member BEFORE it even on a line of its
/// own, so it ends the walk too. Every C++ item is `public: true`, because the
/// spec keeps all visibilities (see `vis_filter`).
fn cpp_lead(node: Node, text: &str) -> Lead {
    let Some(comment) = text
        .get(node.byte_range())
        .filter(|_| node.kind() == "comment")
    else {
        return Lead::Stop;
    };
    if ["///<", "//!<", "/**<", "/*!<"]
        .iter()
        .any(|m| comment.starts_with(m))
    {
        return Lead::Stop;
    }
    // `/***…` is a star box, never Doxygen.
    Some(comment)
        .filter(|c| !c.starts_with("/***"))
        .and_then(cpp_comment_doc)
        .map_or_else(|| Lead::Plain(plain_comment_text(comment)), Lead::Doc)
}

/// Climb to the enclosing `template_declaration`, nested ones included, so a
/// template renders with its head. The exact analogue of Python's
/// `decorated_definition`; without it `template<typename T> class RingBuffer`
/// renders as `class RingBuffer`.
fn cpp_sig_start(def: Node) -> Node {
    outermost(def, |k| k == "template_declaration")
}

/// The outermost `qualified_identifier` around the captured name, or the name
/// itself when it is unqualified.
///
/// This is the byte at which `build_signature` splices the resolved owner in,
/// and C++ is the one language for which it is not the name node: an
/// out-of-line definition writes its own qualifier in the source, so anchoring
/// at the name would INSERT the owner ahead of a qualifier that is already
/// there — `void OrderBook::OrderBook::apply()`. Anchoring at the qualifier
/// makes the owner REPLACE it, which is also what strips the namespace:
/// `void trading::OrderBook::apply()` renders `void OrderBook::apply()`, the
/// same signature the in-class declaration of that method renders.
fn cpp_owner_anchor(name: Node) -> Node {
    outermost(name, |k| k == "qualified_identifier")
}

/// The `operator` of a conversion operator the grammar misread, `name` the
/// name it gave the function: after `inline` or `constexpr`, `K::operator
/// bool()` parses as a function `bool` returning the type `K::operator`.
/// `operator` is a keyword, so a type so named is always this misreading.
/// The node is that type's last name; the scope before it is the class.
pub(crate) fn misread_cast<'t>(name: Node<'t>, text: &str) -> Option<Node<'t>> {
    let written = outermost(name, |k| k == "qualified_identifier");
    let function = written.parent()?;
    let mut ty = (function.parent()?.child_by_field_name("type"))
        .filter(|_| function.kind() == "function_declarator")?;
    while ty.kind() == "qualified_identifier" {
        ty = ty.child_by_field_name("name")?;
    }
    (text.get(ty.byte_range()) == Some("operator")).then_some(ty)
}

/// Reject first, then resolve the owner. The order matters: `int x;` inside a
/// `struct Tmp` declared inside a method has a `field_declaration_list`
/// ancestor, so an owner-first rule would export `int Tmp::x` from a function
/// body. The rejection is load-bearing for a second reason — a most-vexing-parse
/// local such as `std::lock_guard<std::mutex> g(mtx_);` parses as a
/// `declaration` with a `function_declarator` and would otherwise be exported
/// as a function from every `.cpp` that takes a lock.
///
/// The owner chain is enclosing TYPE names, outermost first. Namespaces are
/// excluded — a `namespace_definition` has no `field_declaration_list` — and
/// are likewise stripped from a qualified out-of-line name, because
/// `qualified_owner` keeps only the innermost qualifier and `cpp_owner_anchor`
/// makes it replace the whole written prefix. So `void trading::OrderBook::apply()`
/// and the in-class declaration of the same method both render `OrderBook::apply`
/// and index under one key.
fn cpp_placement(def: Node, text: &str) -> Placement {
    let mut node = def;
    while let Some(parent) = node.parent() {
        if parent.kind() == "compound_statement" {
            return Placement::Rejected;
        }
        node = parent;
    }

    // An out-of-line definition names its owner in the declarator.
    if let Some(owner) = qualified_owner(def, text) {
        return Placement::Scoped(vec![owner]);
    }

    let mut chain = Vec::new();
    let mut node = def;
    while let Some(parent) = node.parent() {
        if parent.kind() == "field_declaration_list" {
            if let Some(name) = parent
                .parent()
                .and_then(|ty| ty.child_by_field_name("name"))
                .and_then(|name| text.get(name.byte_range()))
            {
                chain.push(name.to_string());
            }
        }
        node = parent;
    }
    chain.reverse();
    Placement::Scoped(chain)
}

/// The innermost type qualifier of an out-of-line definition's declarator:
/// `void trading::OrderBook::apply()` -> `OrderBook`. `None` when the
/// declarator is unqualified.
///
/// The grammar labels an ordinary qualifier `namespace_identifier` whether it
/// names a class or a namespace, so the innermost one is the only candidate
/// for a type — which is exactly the one wanted, and is why namespaces need no
/// separate exclusion here.
///
/// Two stated gaps, both observed:
///   * one wrapper level only, and only on a `function_definition`: a
///     pointer- or reference-returning out-of-line definition
///     (`Level* OrderBook::top()`) resolves `OrderBook` through its one
///     `pointer_declarator`/`reference_declarator`, matching the query's
///     wrapper patterns, while `int** OrderBook::grid()` (two wrappers)
///     resolves nothing and is not an item. A definition returning a pointer
///     to a function that returns a pointer (`char *(*get_fn(void))(int)`) is
///     no longer an item either: its wrapped `function_declarator` names a
///     `parenthesized_declarator`, which the general `(_)` capture once
///     extracted as the garbage name `(*get_fn(void))`. The narrowed
///     patterns also miss a pointer-returning definition qualified three or
///     more levels deep (`Level* a::b::Book::top()`);
///   * a template member's qualifier is a `template_type`, so
///     `void RingBuffer<T>::push(T)` owns under `RingBuffer<T>` while the
///     in-class declaration owns under `RingBuffer`. Rust strips generic
///     arguments from an owner for exactly this reason; C++ does not yet, so
///     the two forms of a template method do not index alike.
fn qualified_owner(def: Node, text: &str) -> Option<String> {
    let mut declarator = def.child_by_field_name("declarator")?;
    if def.kind() == "function_definition"
        && matches!(
            declarator.kind(),
            "pointer_declarator" | "reference_declarator"
        )
    {
        declarator = declarator
            .named_children(&mut declarator.walk())
            .find(|c| c.kind() == "function_declarator")?;
    }
    // A conversion operator's `operator_cast` sits under the qualifier
    // itself, with no `function_declarator` around it.
    let inner = if declarator.kind() == "qualified_identifier" {
        declarator
    } else {
        declarator.child_by_field_name("declarator")?
    };
    // `K` of a misread `K::operator bool()`, whatever qualifies `bool`.
    if let Some(operator) = misread_cast(inner, text) {
        let class = operator.parent()?.child_by_field_name("scope")?;
        return text.get(class.byte_range()).map(str::to_string);
    }
    if inner.kind() != "qualified_identifier" {
        return None;
    }
    let mut scope = inner;
    let mut last = None;
    while scope.kind() == "qualified_identifier" {
        if let Some(s) = scope
            .child_by_field_name("scope")
            .and_then(|s| text.get(s.byte_range()))
        {
            last = Some(s.to_string());
        }
        match scope.child_by_field_name("name") {
            Some(next) => scope = next,
            None => break,
        }
    }
    last
}

/// Cut a constructor's signature before its member-initializer list.
///
/// Not redundant with `default_cut`: `field_initializer_list` is a plain child,
/// not the `body` field, so a constructor would otherwise render its entire
/// initializer list — routinely 5-15 wrapped lines — as its signature.
///
/// Nothing else is cut. `OrderBook() = default;` is a `function_definition`
/// whose `default_method_clause` is likewise a plain child, and it must render
/// WHOLE: `= default` is the declaration's entire content.
///
/// A `declaration` is a value by its FIRST declarator, not by the pattern that
/// matched it: `int x, f();` reaches `@def` through `f` yet is a value, as
/// `is_function` reads only the first declarator.
///
/// A variable declaration (an `@item` only, never an export) is a value: it
/// stops at its `=` and re-appends the value within `VALUE_BUDGET`, so a
/// lookup table's initializer never becomes its signature. A brace or paren
/// initializer has no `=`, so it is cut and not re-appended. `int a = 1, b;`
/// renders whole: each name's item shares the one node, and a cut at the
/// first `=` would give `b` the signature `int a = 1`.
pub(crate) fn cpp_shape(def: Node) -> Shape {
    let declarator = def.child_by_field_name("declarator");
    if def.kind() == "declaration" && !declarator.is_some_and(is_function_declarator) {
        let single = def
            .children_by_field_name("declarator", &mut def.walk())
            .count()
            == 1;
        let init = declarator.filter(|d| single && d.kind() == "init_declarator");
        let eq = init.and_then(|d| d.children(&mut d.walk()).find(|c| c.kind() == "="));
        let value = init.and_then(|d| d.child_by_field_name("value"));
        return value_shape(eq, value, true);
    }
    let mut cursor = def.walk();
    // Bound before the struct literal: the child iterator borrows `cursor`,
    // and inside the literal that borrow would outlive it.
    let cut = def
        .children(&mut cursor)
        .find(|c| c.kind() == "field_initializer_list")
        .or_else(|| default_cut(def));
    Shape {
        cut,
        ..default_shape(def)
    }
}

/// True when `d` declares a FUNCTION: a `function_declarator`, a conversion
/// operator's `operator_cast`, or a `function_declarator` wrapped in a single
/// pointer/reference wrapper.
///
/// `extract_code`'s `is_function` gate reads the `declarator` field of a
/// `field_declaration`/`declaration` to tell a C++ header's method
/// declarations (`void apply(const Order&);`) apart from its data members.
/// A member that returns a pointer or a reference does not present a bare
/// `function_declarator` there: `Level* top();` is a `pointer_declarator`
/// wrapping one, and `Level& front();` a `reference_declarator`. Without the
/// descent those members render correctly in `## Exports` yet never reach
/// `Entity::methods`, which `Wiki::search` scores on.
///
/// Exactly ONE wrapper deep, deliberately: that is the reach of the
/// wrapper patterns in `QUERY`, so this gate accepts precisely the members
/// the query captures — `Level** grid()` is uncovered on both sides at once
/// (see `QUERY`'s KNOWINGLY UNCOVERED list), rather than counted as a method
/// that no pattern extracts.
pub(crate) fn is_function_declarator(d: Node) -> bool {
    if matches!(d.kind(), "function_declarator" | "operator_cast") {
        return true;
    }
    if !matches!(d.kind(), "pointer_declarator" | "reference_declarator") {
        return false;
    }
    // `reference_declarator` holds its inner declarator as a plain named
    // child, not under a `declarator` field (the wrapper patterns in `QUERY`
    // spell both without a field name for the same reason), so scan children.
    d.named_children(&mut d.walk())
        .any(|c| c.kind() == "function_declarator")
}

/// The number of parameters in the list right after `name(` in a signature:
/// top-level commas plus one, with `()` and `(void)` counting none. `None`
/// when the signature has no such list.
pub(super) fn arity(signature: &str, name: &str) -> Option<usize> {
    let start = signature.find(&format!("{name}("))? + name.len() + 1;
    let params = &signature[start..];
    let (mut depth, mut commas) = (0i32, 0);
    let mut end = None;
    for (i, c) in params.char_indices() {
        match c {
            '(' | '<' | '[' | '{' => depth += 1,
            ')' if depth == 0 => {
                end = Some(i);
                break;
            }
            ')' | '>' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    let list = params[..end?].trim();
    Some(if list.is_empty() || list == "void" {
        0
    } else {
        commas + 1
    })
}

/// Which side of project-wide pairing `def` may take: a body-less
/// declaration, or a definition that is out of line (no class body around
/// it, so `MyOpts::` in a parameter cannot pass for a qualifier), not
/// `static` (in any specifier position) and not in an anonymous namespace.
/// An out-of-line `X::X() = default;` has no `body` but counts as one: it is
/// the definition. `= delete`, and `= default` inside its class, can have no
/// definition elsewhere: `NoDefinition`. Inline and in-class bodies are
/// neither: they still count toward uniqueness in both pairing steps, so
/// they block but never pair.
pub(crate) fn pair_role(def: Node, text: &str) -> PairRole {
    if def.kind() != "function_definition" {
        return PairRole::Declaration;
    }
    let clause = |k: &str| def.children(&mut def.walk()).any(|c| c.kind() == k);
    let is_static = def
        .children(&mut def.walk())
        .any(|c| c.kind() == "storage_class_specifier" && &text[c.byte_range()] == "static");
    let ancestors = || std::iter::successors(def.parent(), Node::parent);
    let in_class = ancestors().any(|a| a.kind() == "field_declaration_list");
    let anonymous = ancestors()
        .any(|a| a.kind() == "namespace_definition" && a.child_by_field_name("name").is_none());
    let defaulted = clause("default_method_clause");
    if clause("delete_method_clause") || (defaulted && in_class) {
        PairRole::NoDefinition
    } else if (def.child_by_field_name("body").is_none() && !defaulted)
        || is_static
        || in_class
        || anonymous
    {
        PairRole::Neither
    } else {
        PairRole::OutOfLineDefinition
    }
}

/// The innermost named namespace around `def` that is not `inline`, its
/// last segment: what `ns::f()` writes as `ns`. An anonymous or inline
/// namespace is looked through, as a qualified call looks through it.
pub(crate) fn namespace_of(def: Node, text: &str) -> Option<String> {
    std::iter::successors(def.parent(), Node::parent)
        .filter(|a| a.kind() == "namespace_definition")
        .filter(|a| a.child(0).is_none_or(|c| c.kind() != "inline"))
        .find_map(|a| a.child_by_field_name("name"))
        .and_then(|n| text.get(n.byte_range()))
        .and_then(|n| n.rsplit("::").next())
        .map(|n| n.trim().to_string())
}

/// For a tree with errors: the tree of `first_branch(text)` when it is less
/// damaged. Braces split across `#if`/`#else` branches otherwise make a
/// definition run to the end of the file or vanish. The copy has the same
/// bytes and newlines, so its nodes index `text` itself.
pub(crate) fn reparse(parser: &mut Parser, text: &str, tree: &Tree) -> Option<Tree> {
    if !tree.root_node().has_error() {
        return None;
    }
    parser
        .parse(first_branch(text), None)
        .filter(|t| damage(t.root_node()) < damage(tree.root_node()))
}

/// Lines under the outermost ERROR nodes plus one per MISSING node.
fn damage(n: Node) -> usize {
    if n.is_error() {
        return n.end_position().row - n.start_position().row + 1;
    }
    usize::from(n.is_missing()) + n.children(&mut n.walk()).map(damage).sum::<usize>()
}

/// `text` as the preprocessor's first branch, like ctags: conditional
/// directive lines (with their `\` continuations) and every `#elif`/`#else`
/// branch become spaces, newlines kept.
fn first_branch(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Per open conditional: is the branch now being read past the first?
    let mut skip: Vec<bool> = Vec::new();
    let mut continued = false;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let word = body.trim_start().strip_prefix('#').map(|r| {
            r.trim_start()
                .split(|c: char| !c.is_ascii_alphabetic())
                .next()
                .unwrap_or("")
        });
        let directive = continued
            || match word {
                Some("if" | "ifdef" | "ifndef") => {
                    skip.push(false);
                    true
                }
                Some("elif" | "elifdef" | "elifndef" | "else") => {
                    if let Some(top) = skip.last_mut() {
                        *top = true;
                    }
                    true
                }
                Some("endif") => {
                    skip.pop();
                    true
                }
                _ => false,
            };
        if directive || skip.contains(&true) {
            continued = directive && body.ends_with('\\');
            out.extend(std::iter::repeat_n(' ', body.len()));
            out.push_str(&line[body.len()..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests;
