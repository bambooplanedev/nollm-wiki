//! Tests for `extract_cpp`.
use crate::formats::code::testutil::{doc_of, item_lines};

fn syms(path: &str, src: &str) -> Vec<String> {
    crate::extract(path, src).symbols
}

fn summary(path: &str, src: &str) -> Option<String> {
    crate::extract(path, src).summary
}

// Source order; the class first, then each declaration with its return type and
// trailing `const` kept and the owner spliced in; the constructor has no return
// type; the private member is kept.
#[test]
fn declaration_only_header_yields_the_class_and_a_signature_for_each_method() {
    let s = syms(
            "src/orderbook.h",
            "class OrderBook {\npublic:\n  OrderBook(int cap);\n  void apply(const Order& o);\n  int best_bid() const;\nprivate:\n  void rebuild_();\n};\n",
        );
    assert_eq!(
        s,
        [
            "class OrderBook",
            "OrderBook::OrderBook(int cap)",
            "void OrderBook::apply(const Order& o)",
            "int OrderBook::best_bid() const",
            "void OrderBook::rebuild_()",
        ],
        "{s:?}"
    );
}

#[test]
fn out_of_line_definition_renders_the_owner_exactly_once() {
    let s = syms(
        "src/orderbook.cpp",
        "void OrderBook::apply(const Order& o) { (void)o; }\n",
    );
    // Exactly one entry: the owner is spliced once and no duplicate match survives.
    assert_eq!(s, ["void OrderBook::apply(const Order& o)"], "{s:?}");
}

#[test]
fn namespace_qualifier_is_stripped_so_both_forms_index_alike() {
    let s = syms("src/ob.cpp", "void trading::OrderBook::apply() {}\n");
    assert_eq!(s, ["void OrderBook::apply()"], "{s:?}");
}

#[test]
fn constructor_cuts_before_the_member_initializer_list() {
    let s = syms(
        "src/ob.cpp",
        "OrderBook::OrderBook(int cap)\n    : cap_(cap), bids_(cap), seq_(0) { }\n",
    );
    assert_eq!(
        s,
        ["OrderBook::OrderBook(int cap)"],
        "init list leaked: {s:?}"
    );
}

#[test]
fn template_head_is_retained() {
    let s = syms(
        "src/ring.h",
        "template<typename T>\nclass RingBuffer { };\n",
    );
    assert_eq!(
        s,
        ["template<typename T> class RingBuffer"],
        "template head amputated: {s:?}"
    );
}

#[test]
fn destructor_operator_and_defaulted_members() {
    let s = syms(
            "src/ob.h",
            "class OrderBook {\npublic:\n  OrderBook() = default;\n  ~OrderBook();\n  bool operator<(const OrderBook& o) const;\n};\n",
        );
    // Sorted by member name: `OrderBook` < `operator<` < `~OrderBook`.
    assert_eq!(
        s,
        [
            "class OrderBook",
            "OrderBook::OrderBook() = default",
            "bool OrderBook::operator<(const OrderBook& o) const",
            "OrderBook::~OrderBook()",
        ],
        "{s:?}"
    );
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
    assert_eq!(
        s,
        [
            "class B",
            "static B* B::make()",
            "B& B::operator=(const B& o)",
            "Level* B::top()",
        ],
        "{s:?}"
    );
}

#[test]
fn pointer_and_reference_returning_in_class_definitions_are_captured() {
    // pinning: guards the flat `@name` plain-name alternatives inside the
    // wrapper alternation. A nested `[(identifier) (field_identifier) …]
    // @name` there compiles but silently drops these in-class definitions.
    let src = "class B {\npublic:\n  const Stats& stats() const { return s_; }\n  Stats* sp() { return nullptr; }\n};\n";
    let s = syms("src/b.h", src);
    assert_eq!(
        s,
        ["class B", "Stats* B::sp()", "const Stats& B::stats() const"],
        "{s:?}"
    );
    let methods: Vec<String> = crate::extract("src/b.h", src)
        .items
        .into_iter()
        .filter(|i| i.kind == crate::model::CodeItemKind::Method)
        .map(|i| i.name)
        .collect();
    assert_eq!(methods, ["stats", "sp"], "{methods:?}");
}

#[test]
fn a_pointer_returning_out_of_line_definition_renders_its_owner_once() {
    // `qualified_owner` descends one wrapper on a definition, so the
    // `function_definition` wrapper patterns must capture only the last
    // name segment: a general `(_)` capture would also take the whole
    // `qualified_identifier` and splice the owner AHEAD of the qualifier
    // the source already wrote. Asserting presence alone would not catch
    // it — a doubled entry would sit beside a correct one — so the whole
    // list is compared exactly.
    let s = syms(
        "src/ob.cpp",
        "Level* OrderBook::top() { return nullptr; }\n",
    );
    assert_eq!(
        s,
        ["Level* OrderBook::top()"],
        "owner spliced ahead of the source's own qualifier, or doubled: {s:?}"
    );
}

#[test]
fn member_inside_ifdef_is_still_extracted() {
    let s = syms(
        "src/ob.h",
        "class B {\npublic:\n#ifdef NDEBUG\n  void fast();\n#endif\n};\n",
    );
    assert_eq!(s, ["class B", "void B::fast()"], "{s:?}");
}

#[test]
fn nested_class_carries_both_owners() {
    let s = syms(
        "src/ob.h",
        "class Outer {\n  class Inner {\n    void f();\n  };\n};\n",
    );
    assert_eq!(
        s,
        [
            "class Outer",
            "class Outer::Inner",
            "void Outer::Inner::f()"
        ],
        "{s:?}"
    );
}

#[test]
fn function_local_declarations_are_not_exported() {
    let s = syms(
        "src/ob.cpp",
        "void go() {\n  std::lock_guard<std::mutex> g(mtx_);\n  struct Tmp { int x; };\n}\n",
    );
    // Only `go` itself: neither the most-vexing-parse local nor the local type.
    assert_eq!(s, ["void go()"], "{s:?}");
}

#[test]
fn type_aliases_are_captured() {
    let s = syms(
        "src/t.h",
        "using OrderId = unsigned long;\ntypedef unsigned long SeqNo;\n",
    );
    assert_eq!(
        s,
        [
            "using OrderId = unsigned long",
            "typedef unsigned long SeqNo"
        ],
        "{s:?}"
    );
}

#[test]
fn data_members_and_globals_are_not_exported() {
    let s = syms(
            "src/ob.h",
            "class B {\n  int a, b, c;\n  Level bids_[4096];\n  Level* head_;\n  int& r_;\n};\nint g_count;\nint* g_ptr;\n",
        );
    // Only the class: no data member, array, pointer, reference or global.
    assert_eq!(s, ["class B"], "{s:?}");
}

#[test]
fn includes_are_captured_in_both_forms() {
    let e = crate::extract("src/ob.cpp", "#include \"ring.h\"\n#include <vector>\n");
    // Sorted (not in source order): the system form keeps its angle brackets, the quoted form its bare path.
    assert_eq!(e.imports, ["<vector>", "ring.h"], "{:?}", e.imports);
}

#[test]
fn header_members_reach_the_methods_index() {
    let e = crate::extract(
        "src/orderbook.h",
        "class OrderBook {\npublic:\n  void apply(const Order& o);\n  int best_bid() const;\n};\n",
    );
    assert_eq!(e.methods, ["apply", "best_bid"], "methods: {:?}", e.methods);
}

#[test]
fn pointer_and_reference_returning_header_members_reach_the_methods_index() {
    // `Level* top();` is a `field_declaration` whose declarator is a
    // `pointer_declarator` WRAPPING the function declarator, and a
    // `Level& front();` a `reference_declarator`. Both render fine in
    // `## Exports`; without descending through the wrapper they are
    // absent from `methods`, which `Wiki::search` scores on.
    let e = crate::extract(
        "src/orderbook.h",
        "class OrderBook {\npublic:\n  Level* top();\n  Level& front();\n  Level** grid();\n};\n",
    );
    assert_eq!(
        e.symbols,
        [
            "class OrderBook",
            "Level& OrderBook::front()",
            "Level* OrderBook::top()"
        ],
        "{:?}",
        e.symbols
    );
    assert_eq!(e.methods, ["front", "top"], "{:?}", e.methods);
    // `Level** grid()` nests two wrappers, which `QUERY` knowingly does
    // not reach. The two indexes must agree about that: a method the
    // query never captures must not appear in `methods` either.
    // (Both lists above are exact and have no `grid`.)
}

#[test]
fn out_of_line_definition_indexes_the_bare_method_name() {
    let e = crate::extract("src/ob.cpp", "void OrderBook::apply(const Order& o) {}\n");
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
    // with `/**`/`/*!` or plain `/*`. Untracked, the banner's second
    // line would be collected as doc content and concatenated onto the
    // real `///` doc that follows, producing
    // "All rights reserved.\nMaintains the book." instead of the real
    // one-line summary.
    let s = summary(
            "src/f.h",
            "/* Copyright (c) 2026 Acme.\n * All rights reserved.\n */\n/// Maintains the book.\nclass F {};\n",
        );
    assert_eq!(s.as_deref(), Some("Maintains the book."), "got {s:?}");
}

#[test]
fn cpp_plain_comment_above_a_method_is_its_doc() {
    let src = "class F {\n  // true once an open request has been held longer than the ceiling.\n  bool maxWaitExpired() const;\n};\n";
    assert_eq!(
        doc_of("f.h", src, "maxWaitExpired"),
        (
            Some("true once an open request has been held longer than the ceiling.".into()),
            2
        )
    );
}

#[test]
fn plain_above_doxygen_neither_joins_nor_extends() {
    let src = "// Accessors\n/// Best bid.\nint getBids();\n";
    assert_eq!(doc_of("b.h", src, "getBids"), (Some("Best bid.".into()), 2));
}

#[test]
fn rejected_blocks_give_no_doc_and_keep_the_range() {
    for block in [
        "// Getters",
        "// ---- Get X ----",
        "// ======\n// Get X\n// ======",
        "// TODO: make get_x exception free.",
        "// int x = 1;\n// foo();",
        "// Copyright 2024 Acme Corp. All rights reserved.",
    ] {
        let src = format!("int a;\n\n{block}\nint get_x();\n");
        let line = 4 + block.matches('\n').count();
        assert_eq!(doc_of("x.h", &src, "get_x"), (None, line), "{block:?}");
    }
    let src = "// Returns the current x of the cursor.\nint get_x();\n";
    assert_eq!(doc_of("x.h", src, "get_x"), (None, 2));
    // Not a banner: a title line inside a longer block goes, the rest
    // stays; an empty line of a `/* */` block is no divider.
    let src = "int a;\n// --- Get X ---\n// Reads x from the cursor.\nint get_x();\n";
    assert_eq!(
        doc_of("x.h", src, "get_x"),
        (Some("Reads x from the cursor.".into()), 2)
    );
    let src = "int a;\n/*\n * Reads x.\n */\nint get_x();\n";
    assert_eq!(doc_of("x.h", src, "get_x"), (Some("Reads x.".into()), 2));
}

#[test]
fn unicode_framed_titles_and_star_boxes_are_dividers() {
    let src = "int a;\n// ─── Risk limits section ───\nint get_x();\n";
    assert_eq!(doc_of("x.h", src, "get_x"), (None, 3));
    let src = "int a;\n/*****\n * Market Data Feed Handlers\n *****/\nvoid onTick();\n";
    assert_eq!(doc_of("x.h", src, "onTick"), (None, 5));
}

#[test]
fn a_doc_lead_above_a_plain_block_keeps_the_block_out() {
    let src = "/// Best bid.\n// NOLINTNEXTLINE(readability-identifier-naming) because legacy API\nint getBids();\n";
    assert_eq!(doc_of("b.h", src, "getBids"), (None, 3));
}

#[test]
fn a_wrapped_todo_is_no_doc() {
    let src = "int a;\n// TODO(b/123): make this faster once the\n// cache lands.\nint get_x();\n";
    assert_eq!(doc_of("x.h", src, "get_x"), (None, 4));
}

#[test]
fn empty_comment_lines_stay_paragraph_breaks() {
    let src =
        "int a;\n// Reads x from the cursor\n//\n// More detail follows here.\nint get_x();\n";
    assert_eq!(
        doc_of("x.h", src, "get_x"),
        (Some("Reads x from the cursor".into()), 2)
    );
}

#[test]
fn group_label_sharing_a_name_word_is_a_doc() {
    let src = "int a;\n// Risk limits\nint risk_limit();\n";
    assert_eq!(
        doc_of("r.h", src, "risk_limit"),
        (Some("Risk limits".into()), 2)
    );
}

#[test]
fn blank_line_or_trailing_comment_is_not_a_doc() {
    let src = "int a;\n// Frobs the widget.\n\nvoid f();\nint n; // Counts the rows.\nvoid g();\n";
    assert_eq!(doc_of("f.h", src, "f"), (None, 4));
    assert_eq!(doc_of("f.h", src, "g"), (None, 6));
    // A comment trailing the opening brace has no code node beside it.
    let src = "class Book { /// Opens the book.\n  void open();\n};\n";
    assert_eq!(doc_of("b.h", src, "open"), (None, 2));
}

#[test]
fn cpp_items_take_doxygen_docs_and_skip_labels() {
    let src = "class Book {\npublic:\n  /// Applies an order.\n  void apply(int o);\n  // plain\n  int size() const;\n};\n/** Frees all. */\nvoid reset();\n";
    let e = crate::extract("book.h", src);
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
fn namespace_scope_variables_are_value_items_but_not_exports() {
    let table = (1..=30)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let src = format!("namespace app {{\n/// The count.\nint g_count;\nstatic const int K = 3;\nconstexpr int N = 4;\ninline constexpr const char* kName = \"x\";\nint table[] = {{{table}}};\n}}\nclass B {{\n  int a;\n  static constexpr int M = 1;\n}};\nvoid f() {{\n  int local = 0;\n}}\n");
    let e = crate::extract("app.h", &src);
    assert_eq!(
        item_lines(&e.items),
        vec![
            "2-3 value g_count pub",
            "4-4 value K pub",
            "5-5 value N pub",
            "6-6 value kName pub",
            "7-7 value table pub",
            "9-12 type B pub",
            "10-10 value B::a priv",
            "11-11 value B::M priv",
            "13-15 fn f pub",
        ]
    );
    let sigs: Vec<&str> = e.items[..5].iter().map(|i| i.signature.as_str()).collect();
    assert_eq!(
        sigs,
        vec![
            "int g_count",
            "static const int K = 3",
            "constexpr int N = 4",
            "inline constexpr const char* kName = \"x\"",
            "int table[]",
        ]
    );
    assert_eq!(e.items[0].doc.as_deref(), Some("The count."));
    assert_eq!(e.symbols, vec!["class B", "void f()"]);
    assert_eq!(e.defined, vec!["B", "f"]);
}

#[test]
fn a_multi_declarator_variable_renders_its_own_declarator_for_each_name() {
    let e = crate::extract("app.h", "int a = 1, b = 2;\n");
    assert_eq!(
        item_lines(&e.items),
        vec!["1-1 value a pub", "1-1 value b pub"]
    );
    let sigs: Vec<&str> = e.items.iter().map(|i| i.signature.as_str()).collect();
    assert_eq!(sigs, vec!["int a = 1", "int b = 2"]);
}

#[test]
fn a_multi_line_doxygen_block_attaches_and_a_plain_block_does_not() {
    let src = "/**\n * Applies an order\n * to the book. More text.\n */\nvoid apply(int o);\n/* plain block */\nvoid reset();\n";
    let e = crate::extract("book.h", src);
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
    let e = crate::extract("book.h", src);
    assert_eq!(
        item_lines(&e.items),
        vec![
            "1-10 type Book pub",
            "2-2 value Book::size_ priv",
            "3-3 method Book::apply pub",
            "4-4 value Book::count priv",
            "5-6 method Book::reset pub",
            "7-7 value Book::x priv",
            "9-9 method Book::clear pub",
        ]
    );
    let doc = |n: usize| e.items[n].doc.as_deref();
    assert_eq!([doc(1), doc(2), doc(3)], [None, None, None]);
    assert_eq!(doc(4), Some("Resets everything."));
    assert_eq!([doc(5), doc(6)], [None, None]);
}

#[test]
fn arity_counts_top_level_parameters() {
    use super::arity;
    assert_eq!(arity("void Book::apply()", "apply"), Some(0));
    assert_eq!(arity("void apply(void)", "apply"), Some(0));
    assert_eq!(arity("void apply(int o)", "apply"), Some(1));
    assert_eq!(
        arity(
            "void apply(std::map<int, int> m, void (*cb)(int, int), int n = f(1, 2))",
            "apply"
        ),
        Some(3)
    );
    assert_eq!(
        arity("bool operator()(int a, int b) const", "operator()"),
        Some(2)
    );
    assert_eq!(
        arity("KillSwitch::KillSwitch(int a)", "KillSwitch"),
        Some(1)
    );
    assert_eq!(arity("using Cb = int", "Cb"), None);
}

#[test]
fn an_explicit_specialization_is_an_item_named_with_its_arguments() {
    let src = "template <class T> int blank() { return 1; }\ntemplate <>\nint blank<int>() {\n  return 0;\n}\ntemplate <> int blank<long>();\nstruct Lamp { template <class T> int glow(); };\ntemplate <> int Lamp::glow<int>() { return 2; }\n";
    let e = crate::extract("blank.cpp", src);
    assert_eq!(
        item_lines(&e.items),
        vec![
            "1-1 fn blank pub",
            "2-5 fn blank<int> pub",
            "6-6 fn blank<long> pub",
            "7-7 type Lamp pub",
            "7-7 method Lamp::glow pub",
            "8-8 method Lamp::glow<int> pub",
        ]
    );
    let sigs: Vec<&str> = e.items.iter().map(|i| i.signature.as_str()).collect();
    assert_eq!(
        sigs,
        vec![
            "template <class T> int blank()",
            "template <> int blank<int>()",
            "template <> int blank<long>()",
            "struct Lamp",
            "template <class T> int glow()",
            "template <> int Lamp::glow<int>()",
        ]
    );
}

#[test]
fn a_conversion_operator_is_a_method_named_as_written() {
    let src = "struct Gate {\n  operator bool() const {\n    return valid(charge);\n  }\n  explicit operator Kind*() const;\n  operator Kind&();\n};\nGate::operator int() const { return 1; }\n";
    let e = crate::extract("gate.cpp", src);
    assert_eq!(
        item_lines(&e.items),
        vec![
            "1-7 type Gate pub",
            "2-4 method Gate::operator bool pub",
            "5-5 method Gate::operator Kind* pub",
            "6-6 method Gate::operator Kind& pub",
            "8-8 method Gate::operator int pub",
        ]
    );
    assert_eq!(
        e.symbols,
        vec![
            "struct Gate",
            "Gate::operator Kind&()",
            "explicit Gate::operator Kind*() const",
            "Gate::operator bool() const",
            "Gate::operator int() const",
        ]
    );
}

#[test]
fn a_conversion_operator_read_as_a_function_of_the_type_operator_is_its_classs_method() {
    // After `inline` the grammar reads `Vent::operator` as the return type
    // and the converted-to type as the function's name.
    let src = "struct Vent {\n  operator bool() const;\n};\ninline Vent::operator bool() const { return true; }\ninline rig::Vent::operator lib::Text() const { return t; }\nconstexpr Tank<T>::operator Kind() const { return k; }\n";
    let e = crate::extract("vent.h", src);
    assert_eq!(
        item_lines(&e.items),
        vec![
            "1-3 type Vent pub",
            "2-2 method Vent::operator bool pub",
            "4-4 method Vent::operator bool pub",
            "5-5 method Vent::operator lib::Text pub",
            "6-6 method Tank<T>::operator Kind pub",
        ]
    );
    assert_eq!(
        e.symbols,
        vec![
            "constexpr Tank<T>::operator Kind() const",
            "struct Vent",
            "Vent::operator bool() const",
            "inline Vent::operator bool() const",
            "inline rig::Vent::operator lib::Text() const",
        ]
    );
}

// A type's body inside a signature span is elided to `{…}`, so the signature
// still stops at the body. Two shapes put one there: a declaration that
// defines a type in place (`typedef struct … {…} name;`, `static struct
// {…} config;`), and a grammar misparse. `-> decltype(a, b)` derails
// tree-sitter-cpp, which then closes the class early and reads the whole
// class as the return type of the next member, so `m`'s signature was the
// class body (9K characters in nlohmann/json's `json_pointer.hpp`).
#[test]
fn a_type_body_inside_a_signature_is_elided() {
    let src = "template<typename R>\nclass P\n{\n    template<typename B>\n    static auto m(B& o) -> decltype(o.f(), std::declval<B&>())\n    {\n        if (o.x) { return o; }\n        return o;\n    }\n\n    template<typename B>\n    static B& m(B& o)\n    {\n        return o;\n    }\n};\n";
    let e = crate::extract("p.hpp", src);
    let sigs: Vec<&str> = e.items.iter().map(|i| i.signature.as_str()).collect();
    // The misparse is still a misparse, but the class body stays out.
    assert_eq!(
        sigs,
        [
            "template<typename R> class P {…} template<typename B> static B& m(B& o)",
            "class P",
            "template<typename B> static auto m(B& o) -> decltype(o.f()",
        ],
        "body leaked into a signature: {sigs:?}"
    );

    let src = "typedef struct client {\n  int id;\n  char *name;\n} client;\nstatic struct config {\n  int port;\n} config;\nenum { A, B } mode;\n";
    let sigs: Vec<String> = crate::extract("s.h", src)
        .items
        .into_iter()
        .map(|i| i.signature)
        .collect();
    assert_eq!(
        sigs,
        [
            "struct client",
            "typedef struct client {…} client",
            "int id",
            "char *name",
            "struct config",
            "static struct config {…} config",
            "int port",
            "enum {…} mode",
        ],
        "{sigs:?}"
    );
}

#[test]
fn an_overlong_signature_is_cut_with_an_ellipsis() {
    let params = (0..200)
        .map(|i| format!("int a{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let e = crate::extract("f.h", &format!("void f({params});\n"));
    let sig = &e.items[0].signature;
    assert_eq!(sig.chars().count(), 1001, "{sig}");
    assert!(sig.starts_with("void f(int a0, int a1") && sig.ends_with('…'));
}

#[test]
fn a_signature_never_reads_back_a_skipped_branch_or_a_broken_declarator() {
    // A first-branch item reads the copy it was parsed from: the `#else`
    // branch, blank there, once rendered into `sleep`'s signature (reduced
    // from jemalloc's `background_thread.c`, 19K characters).
    let src = "#ifndef X\nbool a(int t) STUB\nvoid b(int t) STUB\n#else\n\tint skipped;\n#  ifndef Y\n#endif\n#endif\nsleep(int t,\n\treturn false;\n";
    // A declaration that runs on into an ERROR stops at its declarator: the
    // class's closing brace once ended the macro's signature.
    let broken = "class B : public A\n{\n    MACRO(B, A, x)\n};\nint main()\n";
    for (path, src, want) in [
        (
            "bt.c",
            src,
            &[
                "void b(int t)",
                "STUB sleep(int t, return false",
                "int skipped",
            ][..],
        ),
        ("m.cpp", broken, &["MACRO(B, A, x)"][..]),
    ] {
        let sigs: Vec<String> = crate::extract(path, src)
            .items
            .into_iter()
            .map(|i| i.signature)
            .collect();
        assert_eq!(sigs, want, "{path}");
    }
}

/// `(owner::name, start-end, signature)` of every nested value item; each one
/// is private.
fn members(path: &str, src: &str) -> Vec<(String, String, String)> {
    let e = crate::extract(path, src);
    e.items
        .iter()
        .filter(|i| i.nested && i.kind == crate::model::CodeItemKind::Value)
        .map(|i| {
            assert!(!i.public, "{} is public", i.name);
            let owner = i
                .owner
                .as_deref()
                .map_or(String::new(), |o| format!("{o}::"));
            (
                format!("{owner}{}", i.name),
                format!("{}-{}", i.start_line, i.end_line),
                i.signature.clone(),
            )
        })
        .collect()
}

fn m(name: &str, lines: &str, sig: &str) -> (String, String, String) {
    (name.to_string(), lines.to_string(), sig.to_string())
}

const WIDGET_H: &str = "\
class Widget {
public:
  void step();
  int count() const { return n_; }
private:
  std::array<uint8_t, 4> pane_border_width_{};
  int n_ = 0;
  double zoom_;
  static constexpr int kMaxPanes = 4;
  static const int kMin = 1;
  Order* head_;
};
";

#[test]
fn class_data_members_are_nested_values_owned_by_the_class() {
    assert_eq!(
        members("widget.h", WIDGET_H),
        vec![
            m(
                "Widget::pane_border_width_",
                "6-6",
                "std::array<uint8_t, 4> pane_border_width_"
            ),
            m("Widget::n_", "7-7", "int n_ = 0"),
            m("Widget::zoom_", "8-8", "double zoom_"),
            m(
                "Widget::kMaxPanes",
                "9-9",
                "static constexpr int kMaxPanes = 4"
            ),
            m("Widget::kMin", "10-10", "static const int kMin = 1"),
            m("Widget::head_", "11-11", "Order* head_"),
        ]
    );
}

#[test]
fn data_members_leave_methods_exports_and_defined_names_as_they_were() {
    let e = crate::extract("widget.h", WIDGET_H);
    let shown: Vec<&String> = e
        .items
        .iter()
        .filter(|i| !i.nested)
        .map(|i| &i.name)
        .collect();
    assert_eq!(shown, ["Widget", "step", "count"]);
    assert_eq!(e.items.iter().filter(|i| i.name == "step").count(), 1);
    assert_eq!(
        e.symbols,
        [
            "class Widget",
            "int Widget::count() const",
            "void Widget::step()"
        ]
    );
    assert_eq!(e.methods, ["count", "step"]);
    assert_eq!(e.defined, ["Widget"]);
}

#[test]
fn plain_struct_fields_are_nested_values_too() {
    let src =
        "struct Cell {\n  int row;\n  double width, height;\n  struct Cell* next;\n  char tag[8];\n};\n";
    assert_eq!(
        members("cell.c", src),
        vec![
            m("Cell::row", "2-2", "int row"),
            m("Cell::height", "3-3", "double height"),
            m("Cell::width", "3-3", "double width"),
            m("Cell::next", "4-4", "struct Cell* next"),
            m("Cell::tag", "5-5", "char tag[8]"),
        ]
    );
}

#[test]
fn anonymous_members_and_bitfields_do_not_panic() {
    let src = "struct P {\n  union { int i; float f; };\n  struct { int a; } s;\n  unsigned flag : 1;\n  class Inner { int deep_; };\n};\nvoid g() { struct T { int local; }; }\n";
    assert_eq!(
        members("p.h", src),
        vec![
            m("P::f", "2-2", "float f"),
            m("P::i", "2-2", "int i"),
            m("P::a", "3-3", "int a"),
            m("P::s", "3-3", "struct {…} s"),
            m("P::flag", "4-4", "unsigned flag : 1"),
            m("P::Inner::deep_", "5-5", "int deep_"),
        ]
    );
}

#[test]
fn pointer_array_and_function_pointer_members_are_nested_values() {
    let src = "struct Ops {
  int (*open_fn)(const char*);
  void (*cb)(int);
  char **argv_list;
  int const* const* pp;
  int grid[3][3];
  char *names[4];
  unsigned bits : 3, more : 2;
  int Box::* pm;
  ATOMIC int ticks;
  int *RESTRICT slots;
  char ***triple;
  void (*handlers[4])(int);
  int (*rows)[3];
  LIST_HEAD(entry_type) link;
};
typedef struct { int anon_field; } Anon;
";
    assert_eq!(
        members("ops.h", src),
        vec![
            m("Ops::open_fn", "2-2", "int (*open_fn)(const char*)"),
            m("Ops::cb", "3-3", "void (*cb)(int)"),
            m("Ops::argv_list", "4-4", "char **argv_list"),
            m("Ops::pp", "5-5", "int const* const* pp"),
            m("Ops::grid", "6-6", "int grid[3][3]"),
            m("Ops::names", "7-7", "char *names[4]"),
            m("Ops::bits", "8-8", "unsigned bits : 3"),
            m("Ops::more", "8-8", "unsigned more : 2"),
            m("Ops::ticks", "10-10", "ATOMIC int ticks"),
            m("Ops::triple", "12-12", "char ***triple"),
            m("Ops::handlers", "13-13", "void (*handlers[4])(int)"),
            m("Ops::rows", "14-14", "int (*rows)[3]"),
            m("Anon::anon_field", "17-17", "int anon_field"),
        ]
    );
}

#[test]
fn methods_constructors_operators_and_aliases_add_no_member() {
    let src = "class Gadget {
public:
  Gadget();
  ~Gadget();
  Gadget& operator=(const Gadget&);
  friend void swap(Gadget&, Gadget&);
  virtual void draw() = 0;
  static Gadget* make();
  enum Mode { kIdle, kBusy };
  using Id = int;
  typedef int Num;
private:
  int (*hook_)(int);
  int spare_, peek();
};
";
    let e = crate::extract("gadget.h", src);
    assert_eq!(
        members("gadget.h", src),
        vec![
            m("Gadget::hook_", "13-13", "int (*hook_)(int)"),
            m("Gadget::spare_", "14-14", "int spare_"),
        ]
    );
    assert!(!e.methods.iter().any(|n| n == "hook_"), "{:?}", e.methods);
    assert!(
        !e.symbols.iter().any(|s| s.contains("hook_")),
        "{:?}",
        e.symbols
    );
}

/// A header of `structs` structs with `fields` bitfields each.
fn generated_header(structs: usize, fields: usize) -> String {
    let mut s = String::new();
    for r in 0..structs {
        s += &format!("struct Block{r} {{\n");
        for b in 0..fields {
            s += &format!("  unsigned slot{r}_{b} : 1;\n");
        }
        s += "};\n";
    }
    s
}

#[test]
fn a_file_past_the_member_cap_indexes_none_of_its_members() {
    let cap = crate::formats::code::MEMBERS_PER_FILE;
    let at = crate::extract("blocks.h", &generated_header(cap / 10, 10));
    assert_eq!(at.items.iter().filter(|i| i.nested).count(), cap);
    assert_eq!(at.members_unindexed, 0);
    let past = crate::extract("blocks.h", &generated_header(cap / 10 + 1, 10));
    assert_eq!(past.items.iter().filter(|i| i.nested).count(), 0);
    assert_eq!(past.members_unindexed, cap + 10);
    // The types stay.
    assert_eq!(past.items.len(), cap / 10 + 1);
}

/// A constructor whose initializer list an `#if` splits parses as a field
/// declaration that also declares a function: its export keeps the shape it
/// had before data members were items.
#[test]
fn a_split_initializer_keeps_its_export_signature() {
    let src = "class Gate {\n public:\n  Gate(int cap)\n      :\n#if !defined(NDEBUG)\n        cap_(cap),\n#endif\n        left_(cap) {\n  }\n private:\n  int cap_;\n  int left_;\n};\n";
    let e = crate::extract("gate.cc", src);
    assert!(
        !e.symbols.iter().any(|s| s.contains("#endif")),
        "{:?}",
        e.symbols
    );
}

/// A constructor behind an unknown macro parses as a field declaration whose
/// initializer list reads as a bitfield width; its item keeps the signature
/// it had before members were items.
#[test]
fn a_macro_led_constructor_keeps_its_item_signature() {
    let src = "template <typename T> struct Knob : Base {\n  const T& ref;\n  tone_t tone;\n  CONSTEXPR_IF Knob(const T& v, tone_t s) : ref(v), tone(s) {}\n};\n";
    let e = crate::extract("knob.h", src);
    let sigs: Vec<&str> = e
        .items
        .iter()
        .filter(|i| !i.nested)
        .map(|i| i.signature.as_str())
        .collect();
    assert!(!sigs.iter().any(|s| s.contains(": ref(v)")), "{sigs:?}");
}

/// Error recovery can insert a field name with no text; it is no member.
#[test]
fn a_name_the_parser_inserted_is_no_item() {
    let src = "#ifndef TINT_H_\n#define TINT_H_\n\n#include \"base.h\"\n\nNS_OPEN\nNS_EXPORT\n\nenum class tint : uint32_t {\n  pale = 0xF0F8FF,\n  dark = 0x000000\n};  // enum class tint\n\nenum class shade : uint8_t {\n  low = 30,\n  high,\n};\n";
    let e = crate::extract("tint.h", src);
    assert!(e.items.iter().all(|i| !i.name.is_empty()), "{:?}", e.items);
}

/// Each item of `src` as `Kind name | signature | start-end`.
fn item_rows(path: &str, src: &str) -> Vec<String> {
    crate::extract(path, src)
        .items
        .iter()
        .map(|i| {
            format!(
                "{:?} {} | {} | {}-{}",
                i.kind, i.name, i.signature, i.start_line, i.end_line
            )
        })
        .collect()
}

#[test]
fn deep_pointer_and_function_pointer_returning_definitions_are_functions() {
    // A definition's declarator nests the function declarator under any
    // number of pointers (`const` between them included), or under the
    // pointer to the function it returns: each is a function, named as
    // written, its signature cut at the body as `char *f(int)`'s is.
    let src = "static char **split_rows(int n)\n{\n  return 0;\n}\n\
               char * const *frozen_rows(int n) { return 0; }\n\
               int ***cube(void) { return 0; }\n\
               static void (*choose_hook(int n))(void)\n{\n  return 0;\n}\n\
               char *(*maker(void))(int) { return 0; }\n";
    assert_eq!(
        item_rows("src/rows.c", src),
        [
            "Fn split_rows | static char **split_rows(int n) | 1-4",
            "Fn frozen_rows | char * const *frozen_rows(int n) | 5-5",
            "Fn cube | int ***cube(void) | 6-6",
            "Fn choose_hook | static void (*choose_hook(int n))(void) | 7-10",
            "Fn maker | char *(*maker(void))(int) | 11-11",
        ]
    );
    let e = crate::extract("src/rows.c", src);
    assert_eq!(
        e.defined,
        ["choose_hook", "cube", "frozen_rows", "maker", "split_rows"]
    );
    // The function's own parameters, not those of the pointer it returns.
    let hook = e.items.iter().find(|i| i.name == "choose_hook").unwrap();
    assert_eq!(hook.arity.map(|a| a.max), Some(Some(1)), "{:?}", hook.arity);
    let maker = e.items.iter().find(|i| i.name == "maker").unwrap();
    assert_eq!(
        maker.arity.map(|a| a.max),
        Some(Some(0)),
        "{:?}",
        maker.arity
    );
}

#[test]
fn deep_pointer_returning_methods_and_reference_to_pointer_returns_are_functions() {
    let src = "class Box {\npublic:\n  char **inline_rows() { return 0; }\n};\n\
               char **Box::rows() { return 0; }\n\
               int *&slot() { static int *p; return p; }\n";
    let e = crate::extract("src/box.cpp", src);
    assert_eq!(
        e.symbols,
        [
            "class Box",
            "char **Box::inline_rows()",
            "char **Box::rows()",
            "int *&slot()"
        ],
        "{:?}",
        e.symbols
    );
    assert_eq!(e.methods, ["inline_rows", "rows"], "{:?}", e.methods);
    assert_eq!(
        item_rows("src/box.cpp", src),
        [
            "Type Box | class Box | 1-4",
            "Method inline_rows | char **inline_rows() | 3-3",
            "Method rows | char **Box::rows() | 5-5",
            "Fn slot | int *&slot() | 6-6",
        ]
    );
}

#[test]
fn neighbours_of_deep_pointer_definitions_stay_as_they_were() {
    // What each read before deep declarators were walked: a variable
    // holding a function pointer, a declaration with no body, K&R
    // definitions (which the grammar reads as a declaration), a macro-led
    // head the grammar cannot parse, and a header's two-star member.
    let cases: [(&str, &str, &[&str]); 7] = [
        ("src/a.c", "void (*handler)(int) = on_tick;\n", &[]),
        ("src/a.c", "void (*spare)(int);\nchar **grid;\n", &[]),
        ("src/a.c", "char **names(void);\n", &[]),
        (
            "src/a.c",
            "char *one_name(void);\n",
            &["Fn one_name | char *one_name(void) | 1-1"],
        ),
        (
            "src/a.c",
            "char **old_style(a)\nint a;\n{ return 0; }\n",
            &[],
        ),
        (
            "src/a.c",
            "WRAP(word) **wrapped(int x) { return 0; }\n",
            &[],
        ),
        (
            "src/a.h",
            "class Box {\npublic:\n  char **rows();\n};\n",
            &["Type Box | class Box | 1-4"],
        ),
    ];
    for (path, src, want) in cases {
        assert_eq!(item_rows(path, src), want, "{src:?}");
        assert!(crate::extract(path, src).methods.is_empty(), "{src:?}");
    }
}
