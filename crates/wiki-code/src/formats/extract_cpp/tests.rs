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
    let src = "int a;\n// about f\n\nvoid f();\nint n; // count\nvoid g();\n";
    assert_eq!(doc_of("f.h", src, "f"), (None, 4));
    assert_eq!(doc_of("f.h", src, "g"), (None, 6));
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
            "3-3 method Book::apply pub",
            "5-6 method Book::reset pub",
            "9-9 method Book::clear pub",
        ]
    );
    assert_eq!(e.items[1].doc, None);
    assert_eq!(e.items[2].doc.as_deref(), Some("Resets everything."));
    assert_eq!(e.items[3].doc, None);
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
