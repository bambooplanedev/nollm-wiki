//! C++: a language whose public surface is a header's declarations, with owner
//! resolution through class bodies and qualified names, and no visibility gate.
use super::code::{
    cpp_doc_line, default_cut, ext_of, keep_all, keep_any_vis, module_stem, no_body_doc,
    no_export_set, no_header_group, source_rank, LangSpec, Lead, Placement, Rank, Shape,
};
use crate::model::{CodeItem, CodeItemKind, Entity};
use std::collections::BTreeMap;
use tree_sitter::Node;

/// Every capture shape a definition's name can take. The general `(_)` form
/// must NOT be used where a qualified-name pattern can also match:
/// tree-sitter emits one match per pattern with no specificity resolution, so
/// a general pattern alongside the qualified-name patterns below would yield
/// two entries for one definition — one with the owner spliced into an
/// already-qualified name — and `dedup_by` would not collapse them, because
/// their signatures differ.
///
/// The six wrapper patterns are the exception, and are general on purpose. They
/// require the `declarator:` field to be a `pointer_declarator` or a
/// `reference_declarator`, where every other pattern requires a bare
/// `function_declarator`, so no node can be matched by both and there is
/// nothing to disambiguate. Spelling out the alternation there instead would
/// need a qualified twin for each, twelve patterns for the same result, and
/// would drop `Level* OrderBook::top()` on the floor in the meantime. The
/// safety of `(_)` rests on `qualified_owner` reaching no owner THROUGH a
/// wrapper — an owner resolved there would be spliced ahead of the qualifier
/// the source already wrote. `a_pointer_returning_out_of_line_definition_renders_its_owner_once`
/// pins that pairing.
///
/// `function_definition` alone covers only members with a BODY, so a
/// declaration-only header would carry nothing but `class OrderBook`;
/// `field_declaration` and `declaration` are what reach the rest, and are also
/// what admit constructors (an `identifier` declarator, because the grammar
/// cannot know the name is the class's), destructors and operators.
///
/// Types and functions are indexed; DATA is not — no member variables, no
/// file-scope globals or constants. `field_declaration.declarator` is
/// `multiple`, so `int a, b, c;` emits three matches over one node, and
/// `default_cut` finds no `body` field: the whole declaration would render
/// three times over, with the owner spliced at a different offset each time. A
/// struct's layout stays visible in the page's `## Body`, which renders the
/// source verbatim.
///
/// The same `multiple` shape recurs for FUNCTIONS, where it is not filtered
/// out: `class B { int f(), *g(); };` is one `field_declaration` with two
/// declarators, so the query emits two matches over that one node, and
/// `default_cut` again finds no `body` field. Both matches render the WHOLE
/// declaration — `int f(), *g()` — with the owner spliced at a different
/// offset in each, rather than each declarator's own slice. Pre-existing and
/// rare; documented, not fixed.
///
/// The same sharing reaches `Entity::methods` for this shape too:
/// `extract_code`'s widened `is_function` check (`code.rs`) reads
/// `def.child_by_field_name("declarator")`, which — because that field is
/// `multiple` — always returns the FIRST declarator on the shared node,
/// regardless of which of the two matches (and which `@name`) is being
/// classified. Harmless in practice now that the check descends a wrapper
/// (`is_function_declarator`): for `int *g(), f();` the first declarator is a
/// `pointer_declarator` around a `function_declarator`, so both matches
/// classify as functions and both `g` and `f` reach `methods`, which is the
/// right answer. It stays a latent mis-read — a declaration mixing a function
/// and a data declarator would classify both alike — and, like the rendering
/// above, is pre-existing and rare: documented, not fixed.
///
/// KNOWINGLY UNCOVERED, each observed against the real grammar:
///   * declarator nesting beyond ONE wrapper — the six wrapper patterns reach
///     exactly one deep, and `int** f()` is
///     `(pointer_declarator (pointer_declarator (function_declarator …)))`;
///   * qualifier nesting beyond two — `a::b::C::m` needs a third pattern;
///   * an out-of-line nested class (`class Outer::Inner { … }`), whose
///     `class_specifier` names itself with a `qualified_identifier`, not the
///     `type_identifier` these patterns require;
///   * template specializations, whose head is a `template_declaration`
///     wrapping a `class_specifier` named by a `template_type`;
///   * a pointer- or reference-returning OUT-OF-LINE definition
///     (`Level* OrderBook::top() { … }`) classifies as `FreeDef`, not
///     `Member`: `qualified_owner` resolves `None` through the wrapper (see
///     its own doc), so `classify` sees no owner and sorts the definition
///     away from its class in `## Exports`, where it also competes in
///     `pick_summary_fallback` against genuine free functions. Closing this
///     needs `qualified_owner` to descend a wrapper level AND the wrapper
///     patterns' `(_)` capture narrowed in the same change — out of scope
///     here;
///   * a conversion operator (`operator bool()`) parses as an `operator_cast`
///     declarator under an `abstract_function_declarator`, not an
///     `operator_name` under a `function_declarator`, so no pattern here
///     reaches it;
///   * a file-scope most-vexing parse (`Level* p(q);` at namespace scope)
///     matches the pointer-declarator wrapper pattern and is exported as a
///     function: `cpp_placement`'s rejection only fires inside a
///     `compound_statement`, which a namespace or file scope is not.
const QUERY: &str = r"
(function_definition declarator: (function_declarator
  declarator: [(identifier) (field_identifier) (destructor_name) (operator_name)] @name)) @def
(field_declaration declarator: (function_declarator
  declarator: [(identifier) (field_identifier) (destructor_name) (operator_name)] @name)) @def
(declaration declarator: (function_declarator
  declarator: [(identifier) (field_identifier) (destructor_name) (operator_name)] @name)) @def

(function_definition declarator: (function_declarator
  declarator: (qualified_identifier
    name: [(identifier) (destructor_name) (operator_name)] @name))) @def
(function_definition declarator: (function_declarator
  declarator: (qualified_identifier name: (qualified_identifier
    name: [(identifier) (destructor_name) (operator_name)] @name)))) @def

(function_definition declarator: (pointer_declarator
  (function_declarator declarator: (_) @name))) @def
(function_definition declarator: (reference_declarator
  (function_declarator declarator: (_) @name))) @def
(field_declaration declarator: (pointer_declarator
  (function_declarator declarator: (_) @name))) @def
(field_declaration declarator: (reference_declarator
  (function_declarator declarator: (_) @name))) @def
(declaration declarator: (pointer_declarator
  (function_declarator declarator: (_) @name))) @def
(declaration declarator: (reference_declarator
  (function_declarator declarator: (_) @name))) @def

(class_specifier   name: (type_identifier) @name) @def
(struct_specifier  name: (type_identifier) @name) @def
(union_specifier   name: (type_identifier) @name) @def
(enum_specifier    name: (type_identifier) @name) @def
(alias_declaration name: (type_identifier) @name) @def
(type_definition   declarator: (type_identifier) @name) @def

(preproc_include path: (_) @import)
";

pub(crate) fn cpp_spec() -> LangSpec {
    LangSpec {
        lang_name: "cpp",
        language: tree_sitter_cpp::LANGUAGE.into(),
        query_src: QUERY,
        name_filter: keep_all,
        // Everything visible is extracted, private members included. Gating on
        // `access_specifier` would be inconsistent — an out-of-line
        // `void OrderBook::rebuild_()` carries no visible specifier and would
        // be kept anyway — and a sticky previous-sibling scan would default to
        // private and drop every member nested in a `template_declaration` or
        // a `preproc_ifdef`, neither of which has a sibling specifier in the
        // `field_declaration_list`.
        vis_filter: keep_any_vis,
        // `OrderBook() = default;` renders whole: `default_method_clause` is
        // not the `body` field, so no cut applies. `declaration` and
        // `field_declaration` also carry their `;`.
        strip_trailing: &[';'],
        placement: cpp_placement,
        owner_sep: "::",
        export_set: no_export_set,
        join_continuations: false,
        sig_start: cpp_sig_start,
        owner_anchor: cpp_owner_anchor,
        header_group: no_header_group,
        item_lead: cpp_lead,
        body_doc: no_body_doc,
        query_gates_visibility: false,
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

/// A Doxygen comment directly above an item is its doc; anything else ends the
/// walk. A member-trailing doc (`///<`, `//!<`, `/**<`, `/*!<`) documents the
/// member BEFORE it even on a line of its own, so it ends the walk too. Every
/// C++ item is `public: true`, because the spec keeps all visibilities (see
/// `vis_filter`).
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
    cpp_comment_doc(comment).map_or(Lead::Stop, Lead::Doc)
}

/// Climb to the enclosing `template_declaration`, nested ones included, so a
/// template renders with its head. The exact analogue of Python's
/// `decorated_definition`; without it `template<typename T> class RingBuffer`
/// renders as `class RingBuffer`.
pub(crate) fn cpp_sig_start(def: Node) -> Node {
    let mut node = def;
    while let Some(parent) = node.parent() {
        if parent.kind() == "template_declaration" {
            node = parent;
        } else {
            break;
        }
    }
    node
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
pub(crate) fn cpp_owner_anchor(name: Node) -> Node {
    let mut node = name;
    while let Some(parent) = node.parent() {
        if parent.kind() == "qualified_identifier" {
            node = parent;
        } else {
            break;
        }
    }
    node
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
pub(crate) fn cpp_placement(def: Node, text: &str) -> Placement {
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
///   * one declarator level only, so a pointer- or reference-returning
///     out-of-line definition (`Level* OrderBook::top()`) resolves no owner and
///     renders its qualifier verbatim instead — matching the query's own
///     wrapper patterns;
///   * a template member's qualifier is a `template_type`, so
///     `void RingBuffer<T>::push(T)` owns under `RingBuffer<T>` while the
///     in-class declaration owns under `RingBuffer`. Rust strips generic
///     arguments from an owner for exactly this reason; C++ does not yet, so
///     the two forms of a template method do not index alike.
fn qualified_owner(def: Node, text: &str) -> Option<String> {
    let declarator = def.child_by_field_name("declarator")?;
    let inner = declarator.child_by_field_name("declarator")?;
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
pub(crate) fn cpp_shape(def: Node) -> Shape {
    let mut cursor = def.walk();
    // Bound before the struct literal: the child iterator borrows `cursor`,
    // and inside the literal that borrow would outlive it.
    let cut = def
        .children(&mut cursor)
        .find(|c| c.kind() == "field_initializer_list")
        .or_else(|| default_cut(def));
    Shape {
        rank: Rank::Def,
        cut,
        value: None,
        has_type: true,
    }
}

/// True when `d` declares a FUNCTION: a `function_declarator`, or one wrapped
/// in a single pointer/reference wrapper.
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
/// Exactly ONE wrapper deep, deliberately: that is the reach of the six
/// wrapper patterns in `QUERY`, so this gate accepts precisely the members
/// the query captures — `Level** grid()` is uncovered on both sides at once
/// (see `QUERY`'s KNOWINGLY UNCOVERED list), rather than counted as a method
/// that no pattern extracts.
pub(crate) fn is_function_declarator(d: Node) -> bool {
    if d.kind() == "function_declarator" {
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

/// The `(owner, name)` keys that occur exactly once among `items`, with the
/// index of their item. Overloads share a key, so they drop out here.
fn unique_keys(items: &[CodeItem]) -> BTreeMap<(Option<&str>, &str), usize> {
    let mut seen: BTreeMap<(Option<&str>, &str), Option<usize>> = BTreeMap::new();
    for (i, it) in items.iter().enumerate() {
        if matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method) {
            seen.entry((it.owner.as_deref(), it.name.as_str()))
                .and_modify(|v| *v = None)
                .or_insert(Some(i));
        }
    }
    seen.into_iter()
        .filter_map(|(k, v)| v.map(|i| (k, i)))
        .collect()
}

/// Pairs a header's declarations with the definitions in the one `.cpp` that
/// implements it: the doc usually sits on the declaration, so the definition
/// inherits it, and each side's `twin` names the other's lines. Only a
/// group with exactly one implementation file and a unique lowest-rank header
/// pairs, and only keys unique on both sides: a wrong pairing is worse than
/// none, so overloads and ambiguous groups stay untouched.
pub(crate) fn pair_declarations(entities: &mut BTreeMap<String, Entity>) {
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, e) in entities.iter() {
        let sp = e.source_path.as_str();
        if matches!(
            ext_of(sp).as_str(),
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "h"
        ) {
            groups.entry(module_stem(sp)).or_default().push(id);
        }
    }
    let mut pairs: Vec<(String, usize, String, usize)> = Vec::new();
    for ids in groups.values() {
        let impls: Vec<&str> = ids
            .iter()
            .copied()
            .filter(|id| source_rank(&entities[*id].source_path) == 2)
            .collect();
        let Some(min) = ids
            .iter()
            .map(|id| source_rank(&entities[*id].source_path))
            .min()
            .filter(|r| *r < 2)
        else {
            continue;
        };
        let headers: Vec<&str> = ids
            .iter()
            .copied()
            .filter(|id| source_rank(&entities[*id].source_path) == min)
            .collect();
        let ([imp], [hdr]) = (impls.as_slice(), headers.as_slice()) else {
            continue;
        };
        let (h, c) = (&entities[*hdr].items, &entities[*imp].items);
        let cu = unique_keys(c);
        for (key, hi) in unique_keys(h) {
            if let Some(&ci) = cu.get(&key) {
                pairs.push(((*hdr).to_string(), hi, (*imp).to_string(), ci));
            }
        }
    }
    for (hid, hi, cid, ci) in pairs {
        let span = |e: &Entity, i: usize| {
            let it = &e.items[i];
            format!("{}:{}-{}", e.source_path, it.start_line, it.end_line)
        };
        let (h_span, c_span) = (span(&entities[&hid], hi), span(&entities[&cid], ci));
        let doc = entities[&hid].items[hi].doc.clone();
        let h_item = &mut entities.get_mut(&hid).unwrap().items[hi];
        h_item.twin = Some(c_span);
        let c_item = &mut entities.get_mut(&cid).unwrap().items[ci];
        c_item.twin = Some(h_span);
        if c_item.doc.is_none() {
            c_item.doc = doc;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::formats::code::testutil::item_lines;
    use crate::formats::code::CodeExtractor;
    use crate::formats::Extractor;

    fn syms(path: &str, src: &str) -> Vec<String> {
        CodeExtractor.extract(path, src).symbols
    }

    fn summary(path: &str, src: &str) -> Option<String> {
        CodeExtractor.extract(path, src).summary
    }

    #[test]
    fn declaration_only_header_yields_one_signature_per_member() {
        let s = syms(
            "src/orderbook.h",
            "class OrderBook {\npublic:\n  OrderBook(int cap);\n  void apply(const Order& o);\n  int best_bid() const;\nprivate:\n  void rebuild_();\n};\n",
        );
        assert!(s.iter().any(|x| x == "class OrderBook"), "{s:?}");
        assert!(s.iter().any(|x| x.contains("OrderBook::apply")), "{s:?}");
        assert!(s.iter().any(|x| x.contains("OrderBook::best_bid")), "{s:?}");
        assert!(
            s.iter().any(|x| x.contains("OrderBook::rebuild_")),
            "private members are kept: {s:?}"
        );
    }

    #[test]
    fn out_of_line_definition_renders_the_owner_exactly_once() {
        let s = syms(
            "src/orderbook.cpp",
            "void OrderBook::apply(const Order& o) { (void)o; }\n",
        );
        assert!(
            s.iter()
                .any(|x| x == "void OrderBook::apply(const Order& o)"),
            "{s:?}"
        );
        assert!(
            !s.iter().any(|x| x.contains("OrderBook::OrderBook::")),
            "owner spliced twice, or a duplicate match survived: {s:?}"
        );
        assert_eq!(s.iter().filter(|x| x.contains("apply")).count(), 1, "{s:?}");
    }

    #[test]
    fn namespace_qualifier_is_stripped_so_both_forms_index_alike() {
        let s = syms("src/ob.cpp", "void trading::OrderBook::apply() {}\n");
        assert!(s.iter().any(|x| x == "void OrderBook::apply()"), "{s:?}");
    }

    #[test]
    fn constructor_cuts_before_the_member_initializer_list() {
        let s = syms(
            "src/ob.cpp",
            "OrderBook::OrderBook(int cap)\n    : cap_(cap), bids_(cap), seq_(0) { }\n",
        );
        assert!(
            s.iter().any(|x| x == "OrderBook::OrderBook(int cap)"),
            "{s:?}"
        );
        assert!(
            !s.iter().any(|x| x.contains("cap_(cap)")),
            "init list leaked: {s:?}"
        );
    }

    #[test]
    fn template_head_is_retained() {
        let s = syms(
            "src/ring.h",
            "template<typename T>\nclass RingBuffer { };\n",
        );
        assert!(
            s.iter().any(|x| x.starts_with("template<typename T>")),
            "template head amputated: {s:?}"
        );
    }

    #[test]
    fn destructor_operator_and_defaulted_members() {
        let s = syms(
            "src/ob.h",
            "class OrderBook {\npublic:\n  OrderBook() = default;\n  ~OrderBook();\n  bool operator<(const OrderBook& o) const;\n};\n",
        );
        assert!(s.iter().any(|x| x.contains("= default")), "{s:?}");
        assert!(s.iter().any(|x| x.contains("~OrderBook")), "{s:?}");
        assert!(s.iter().any(|x| x.contains("operator<")), "{s:?}");
    }

    #[test]
    fn pointer_return_type_is_captured() {
        let s = syms(
            "src/ob.cpp",
            "Level* OrderBook::top() { return nullptr; }\n",
        );
        assert!(s.iter().any(|x| x.contains("OrderBook::top")), "{s:?}");
    }

    #[test]
    fn pointer_and_reference_returning_member_declarations_are_captured() {
        // Factory and accessor methods returning a handle are most of what a
        // real header declares, and each nests its `function_declarator` one
        // level down, inside a `pointer_declarator` or a
        // `reference_declarator`. Without the wrapper patterns on
        // `field_declaration` a header page silently loses its whole
        // pointer-returning API.
        let s = syms(
            "src/b.h",
            "class B {\npublic:\n  Level* top();\n  static B* make();\n  B& operator=(const B& o);\n};\n",
        );
        assert!(s.iter().any(|x| x.contains("B::top")), "{s:?}");
        assert!(s.iter().any(|x| x.contains("B::make")), "{s:?}");
        assert!(s.iter().any(|x| x.contains("operator=")), "{s:?}");
    }

    #[test]
    fn a_pointer_returning_out_of_line_definition_renders_its_owner_once() {
        // The wrapper patterns capture a general `(_)` name, so on an
        // out-of-line definition the capture is the whole
        // `qualified_identifier`. That is safe only while `qualified_owner`
        // reaches no owner through a wrapper: were it to, the owner would be
        // spliced in AHEAD of the qualifier the source already wrote. Asserting
        // presence alone would not catch it — a doubled entry would sit beside
        // a correct one — so the absence is asserted too.
        let s = syms(
            "src/ob.cpp",
            "Level* OrderBook::top() { return nullptr; }\n",
        );
        assert!(s.iter().any(|x| x.contains("OrderBook::top")), "{s:?}");
        assert!(
            !s.iter().any(|x| x.contains("OrderBook::OrderBook::")),
            "owner spliced ahead of the source's own qualifier: {s:?}"
        );
        assert_eq!(s.iter().filter(|x| x.contains("top")).count(), 1, "{s:?}");
    }

    #[test]
    fn member_inside_ifdef_is_still_extracted() {
        let s = syms(
            "src/ob.h",
            "class B {\npublic:\n#ifdef NDEBUG\n  void fast();\n#endif\n};\n",
        );
        assert!(s.iter().any(|x| x.contains("B::fast")), "{s:?}");
    }

    #[test]
    fn nested_class_carries_both_owners() {
        let s = syms(
            "src/ob.h",
            "class Outer {\n  class Inner {\n    void f();\n  };\n};\n",
        );
        assert!(s.iter().any(|x| x.contains("Outer::Inner::f")), "{s:?}");
    }

    #[test]
    fn function_local_declarations_are_not_exported() {
        let s = syms(
            "src/ob.cpp",
            "void go() {\n  std::lock_guard<std::mutex> g(mtx_);\n  struct Tmp { int x; };\n}\n",
        );
        assert!(
            !s.iter().any(|x| x.contains("lock_guard")),
            "most-vexing-parse local exported: {s:?}"
        );
        assert!(
            !s.iter().any(|x| x.contains("Tmp")),
            "function-local type exported: {s:?}"
        );
        assert!(s.iter().any(|x| x.contains("go")), "{s:?}");
    }

    #[test]
    fn type_aliases_are_captured() {
        let s = syms(
            "src/t.h",
            "using OrderId = unsigned long;\ntypedef unsigned long SeqNo;\n",
        );
        assert!(s.iter().any(|x| x.contains("OrderId")), "{s:?}");
        assert!(s.iter().any(|x| x.contains("SeqNo")), "{s:?}");
    }

    #[test]
    fn data_members_and_globals_are_not_exported() {
        let s = syms(
            "src/ob.h",
            "class B {\n  int a, b, c;\n  Level bids_[4096];\n  Level* head_;\n  int& r_;\n};\nint g_count;\nint* g_ptr;\n",
        );
        assert!(
            !s.iter().any(|x| x.contains("a, b, c")),
            "data members leaked: {s:?}"
        );
        assert!(
            !s.iter().any(|x| x.contains("bids_")),
            "array field leaked: {s:?}"
        );
        assert!(
            !s.iter().any(|x| x.contains("head_")),
            "pointer field leaked: {s:?}"
        );
        assert!(
            !s.iter().any(|x| x.contains("r_")),
            "reference field leaked: {s:?}"
        );
        assert!(
            !s.iter().any(|x| x.contains("g_count")),
            "global leaked: {s:?}"
        );
        assert!(
            !s.iter().any(|x| x.contains("g_ptr")),
            "pointer global leaked: {s:?}"
        );
        assert!(s.iter().any(|x| x == "class B"), "{s:?}");
    }

    #[test]
    fn includes_are_captured_in_both_forms() {
        let e = CodeExtractor.extract("src/ob.cpp", "#include <vector>\n#include \"ring.h\"\n");
        assert!(
            e.imports.iter().any(|i| i.contains("ring.h")),
            "{:?}",
            e.imports
        );
        assert!(
            e.imports.iter().any(|i| i.contains("vector")),
            "{:?}",
            e.imports
        );
    }

    #[test]
    fn header_members_reach_the_methods_index() {
        let e = CodeExtractor.extract(
            "src/orderbook.h",
            "class OrderBook {\npublic:\n  void apply(const Order& o);\n  int best_bid() const;\n};\n",
        );
        assert!(
            e.methods.iter().any(|m| m == "apply"),
            "methods: {:?}",
            e.methods
        );
        assert!(
            e.methods.iter().any(|m| m == "best_bid"),
            "methods: {:?}",
            e.methods
        );
    }

    #[test]
    fn pointer_and_reference_returning_header_members_reach_the_methods_index() {
        // `Level* top();` is a `field_declaration` whose declarator is a
        // `pointer_declarator` WRAPPING the function declarator, and a
        // `Level& front();` a `reference_declarator`. Both render fine in
        // `## Exports`; without descending through the wrapper they are
        // absent from `methods`, which `Wiki::search` scores on.
        let e = CodeExtractor.extract(
            "src/orderbook.h",
            "class OrderBook {\npublic:\n  Level* top();\n  Level& front();\n  Level** grid();\n};\n",
        );
        for m in ["top", "front"] {
            assert!(
                e.methods.iter().any(|x| x == m),
                "{m} missing from methods: {:?}",
                e.methods
            );
            assert!(
                e.symbols.iter().any(|s| s.contains(m)),
                "{m} missing from symbols: {:?}",
                e.symbols
            );
        }
        // `Level** grid()` nests two wrappers, which `QUERY` knowingly does
        // not reach. The two indexes must agree about that: a method the
        // query never captures must not appear in `methods` either.
        assert!(
            !e.symbols.iter().any(|s| s.contains("grid")),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            !e.methods.iter().any(|x| x == "grid"),
            "methods must not outrun what `## Exports` captures: {:?}",
            e.methods
        );
    }

    #[test]
    fn out_of_line_definition_indexes_the_bare_method_name() {
        let e = CodeExtractor.extract("src/ob.cpp", "void OrderBook::apply(const Order& o) {}\n");
        assert_eq!(
            e.methods,
            vec!["apply".to_string()],
            "methods: {:?}",
            e.methods
        );
    }

    #[test]
    fn doxygen_block_on_its_own_line_summarises() {
        let s = summary(
            "src/ob.h",
            "/**\n * @brief Maintains the L2 order book.\n */\nclass OrderBook {};\n",
        );
        assert_eq!(
            s.as_deref(),
            Some("Maintains the L2 order book."),
            "got {s:?}"
        );
    }

    #[test]
    fn triple_slash_and_bang_forms_summarise() {
        assert_eq!(
            summary("src/a.h", "/// Maintains the book.\nclass A {};\n").as_deref(),
            Some("Maintains the book.")
        );
        assert_eq!(
            summary("src/b.h", "/*! Maintains the book. */\nclass B {};\n").as_deref(),
            Some("Maintains the book.")
        );
    }

    #[test]
    fn banners_never_become_summaries() {
        // Each file's only comment is banner-shaped, never a real doc
        // marker, so `leading_doc` finds no docstring and the summary falls
        // back to the class header — the exact outcome, not merely the
        // absence of the bad string the banner itself would have produced.
        let llvm = summary(
            "src/c.h",
            "//===--- OrderBook.h - L2 book ----*- C++ -*-===//\nclass C {};\n",
        );
        assert_eq!(llvm.as_deref(), Some("class C"), "got {llvm:?}");

        let stars = summary("src/d.h", "/*****************/\nclass D {};\n");
        assert_eq!(stars.as_deref(), Some("class D"), "got {stars:?}");

        let plain = summary("src/e.h", "// Copyright (c) 2026 Acme.\nclass E {};\n");
        assert_eq!(plain.as_deref(), Some("class E"), "got {plain:?}");
    }

    #[test]
    fn real_code_between_a_banner_and_a_doc_comment_stops_the_search() {
        // The C++ banner-skip must tell "comment we can skip past" apart
        // from "code that ends the search". A member's own doc comment,
        // reached only by walking through `class OrderBook {` and
        // `public:`, must never be reported as the FILE's summary — that is
        // the same buried-doc defect the non-C++ languages were fixed
        // against, merely narrowed to needing real code in between rather
        // than eliminated.
        let s = summary(
            "src/g.h",
            "class OrderBook {\npublic:\n  /// Applies an order to the book.\n  void apply(const Order& o);\n};\n",
        );
        assert_ne!(
            s.as_deref(),
            Some("Applies an order to the book."),
            "a member's doc comment was reported as the file's summary: {s:?}"
        );
        assert_eq!(s.as_deref(), Some("class OrderBook"), "got {s:?}");
    }

    #[test]
    fn doc_comment_below_a_license_banner_is_still_found() {
        let s = summary(
            "src/f.h",
            "// Copyright (c) 2026 Acme.\n// SPDX-License-Identifier: MIT\n\n/// Maintains the book.\nclass F {};\n",
        );
        assert_eq!(s.as_deref(), Some("Maintains the book."), "got {s:?}");
    }

    #[test]
    fn a_c_style_license_block_does_not_bleed_into_the_doc_comment_below_it() {
        // A `/* ... */` banner's `*`-prefixed continuation lines (`* All
        // rights reserved.`) look identical, line by line, to a Doxygen
        // `/** ... */` block's own continuation lines — the only thing that
        // tells them apart is whether the block that opened them started
        // with `/**`/`/*!` or plain `/*`. Before this was tracked, the
        // banner's second line was collected as if it were doc content and
        // concatenated onto the real `///` doc that follows, producing
        // "All rights reserved.\nMaintains the book." instead of the real
        // one-line summary.
        let s = summary(
            "src/f.h",
            "/* Copyright (c) 2026 Acme.\n * All rights reserved.\n */\n/// Maintains the book.\nclass F {};\n",
        );
        assert_eq!(s.as_deref(), Some("Maintains the book."), "got {s:?}");
    }

    #[test]
    fn cpp_items_take_doxygen_docs_and_skip_plain_comments() {
        let src = "class Book {\npublic:\n  /// Applies an order.\n  void apply(int o);\n  // plain\n  int size() const;\n};\n/** Frees all. */\nvoid reset();\n";
        let e = CodeExtractor.extract("book.h", src);
        assert_eq!(
            item_lines(&e.items),
            vec![
                "1-7 type Book pub",
                "3-4 method Book::apply pub",
                "6-6 method Book::size pub",
                "8-9 fn reset pub",
            ]
        );
        assert_eq!(e.items[1].doc.as_deref(), Some("Applies an order."));
        assert_eq!(e.items[2].doc, None);
        assert_eq!(e.items[3].doc.as_deref(), Some("Frees all."));
    }

    #[test]
    fn a_multi_line_doxygen_block_attaches_and_a_plain_block_does_not() {
        let src = "/**\n * Applies an order\n * to the book. More text.\n */\nvoid apply(int o);\n/* plain block */\nvoid reset();\n";
        let e = CodeExtractor.extract("book.h", src);
        assert_eq!(
            item_lines(&e.items),
            vec!["1-5 fn apply pub", "7-7 fn reset pub"]
        );
        assert_eq!(
            e.items[0].doc.as_deref(),
            Some("Applies an order to the book.")
        );
        assert_eq!(e.items[1].doc, None);
    }

    #[test]
    fn a_trailing_member_doc_never_documents_the_next_item() {
        let src = "class Book {\n  int size_; ///< Number of orders.\n  void apply(int o);\n  int count; //!< Count.\n  /** Resets everything. */\n  void reset();\n  int x;\n  ///< The x.\n  void clear();\n};\n";
        let e = CodeExtractor.extract("book.h", src);
        assert_eq!(
            item_lines(&e.items),
            vec![
                "1-10 type Book pub",
                "3-3 method Book::apply pub",
                "5-6 method Book::reset pub",
                "9-9 method Book::clear pub",
            ]
        );
        assert_eq!(e.items[1].doc, None);
        assert_eq!(e.items[2].doc.as_deref(), Some("Resets everything."));
        assert_eq!(e.items[3].doc, None);
    }
}
