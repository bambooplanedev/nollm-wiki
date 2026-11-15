//! Tests for `extract_python`.
use crate::formats::code::testutil::{assert_has, assert_lacks, doc_of, item_lines};

#[test]
fn a_unicode_framed_comment_title_is_no_doc() {
    let src = "x = 1\n# ─── indexing helpers ───\ndef build():\n    pass\n";
    let e = crate::extract("b.py", src);
    let item = e.items.iter().find(|i| i.name == "build").unwrap();
    assert_eq!((item.doc.clone(), item.start_line), (None, 3));
}

#[test]
fn defined_excludes_dataclass_fields_and_keeps_class_def_constant() {
    let src = "from dataclasses import dataclass\n\n@dataclass\nclass Invoice:\n    total: float\n\ndef render():\n    pass\n\nDEFAULT = 3\n";
    let e = crate::extract("billing.py", src);
    assert_eq!(
        e.defined,
        vec!["DEFAULT", "Invoice", "render"],
        "defined: {:?}",
        e.defined
    );
}

#[test]
fn python_signatures_gated_docstring_and_imports() {
    let src = "\"\"\"\nTop docstring.\n\"\"\"\nimport os\nfrom graph import build\ndef extract_all(d):\n    pass\ndef _private():\n    pass\n";
    let e = crate::extract("extractor.py", src);
    assert_has(&e.symbols, "def extract_all(d)");
    assert_lacks(&e.symbols, "_private");
    assert_eq!(e.imports, vec!["graph", "os"]);
    assert_eq!(e.summary.as_deref(), Some("Top docstring."));
}

#[test]
fn a_docstring_sentence_wrapped_across_lines_summarizes_whole() {
    let src = "\"\"\"\nFetch feeds over HTTP with retries and a\nbounded cache. Second sentence.\n\nMore.\n\"\"\"\n";
    let e = crate::extract("fetch.py", src);
    assert_eq!(
        e.summary.as_deref(),
        Some("Fetch feeds over HTTP with retries and a bounded cache.")
    );
}

#[test]
fn a_boilerplate_first_paragraph_is_not_replaced_by_the_second() {
    // The opening paragraph is boilerplate, so the docstring gives no
    // summary; the second paragraph must not stand in for it.
    let src = "\"\"\"\nThis module fetches feeds.\n\nIt retries on failure.\n\"\"\"\ndef fetch():\n    pass\n";
    let e = crate::extract("fetch.py", src);
    assert_eq!(e.summary.as_deref(), Some("def fetch()"));
}

#[test]
fn summary_fallback_prefers_a_definition_over_a_constant() {
    let src =
        "USER_AGENT = \"Mozilla/5.0\"\n\ndef fetch_feed(url: str) -> bytes:\n    return b\"\"\n";
    let e = crate::extract("fetch.py", src);
    assert_eq!(
        e.summary.as_deref(),
        Some("def fetch_feed(url: str) -> bytes"),
        "an uppercase constant sorts first and must not win: {:?}",
        e.symbols
    );
}

#[test]
fn summary_fallback_prefers_a_class_over_its_own_fields() {
    let src = "@dataclass(frozen=True)\nclass Article:\n    id: str\n    title: str\n";
    let e = crate::extract("models.py", src);
    assert_eq!(
        e.summary.as_deref(),
        Some("@dataclass(frozen=True) class Article"),
        "symbols: {:?}",
        e.symbols
    );
}

#[test]
fn python_relative_and_aliased_imports_are_captured() {
    let src = "import os\nimport aggregator.main as m\nfrom graph import build\nfrom .models import Article, FeedSource\nfrom . import state\nfrom . import helpers as h\nfrom ..pkg.sub import thing\n";
    let e = crate::extract("main.py", src);
    assert_eq!(
        e.imports,
        vec![
            "..pkg.sub".to_string(),
            ".models".to_string(),
            "aggregator.main".to_string(),
            "graph".to_string(),
            "helpers".to_string(),
            "os".to_string(),
            "state".to_string(),
        ],
        "imports: {:?}",
        e.imports
    );
}

#[test]
fn python_methods_are_qualified_by_their_class() {
    let src = "class Wiki:\n    def search(self, q: str) -> list:\n        return []\n    async def fetch(self) -> bytes:\n        return b\"\"\n\ndef free_function() -> int:\n    return 1\n";
    let e = crate::extract("q.py", src);
    assert_has(&e.symbols, "def Wiki.search(self, q: str) -> list");
    assert_has(&e.symbols, "async def Wiki.fetch(self) -> bytes");
    assert_has(&e.symbols, "def free_function() -> int");
}

#[test]
fn python_nested_class_chain_is_outermost_first() {
    let src =
        "class Article:\n    class Inner:\n        def deep(self) -> None:\n            pass\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "class Article.Inner");
    assert!(
        e.symbols
            .iter()
            .any(|s| s == "def Article.Inner.deep(self) -> None"),
        "chain must be outermost-first, not Inner.Article: {:?}",
        e.symbols
    );
}

#[test]
fn python_private_class_takes_its_methods_with_it() {
    // A public sibling class makes this test self-guarding: asserting only
    // `symbols.is_empty()` would equally pass if Python extraction were
    // disabled entirely. `Public` must survive so the empty methods of
    // `_TextExtractor` are known to have been gated, not simply never
    // extracted.
    let src = "class _TextExtractor:\n    def handle_data(self, data: str) -> None:\n        pass\n    def text(self) -> str:\n        return \"\"\n\nclass Public:\n    def run(self) -> None:\n        pass\n";
    let e = crate::extract("parse.py", src);
    assert!(
        !e.symbols.iter().any(|s| s.contains("_TextExtractor")
            || s.contains("handle_data")
            || s.contains("text(")),
        "a private class must not export its methods: {:?}",
        e.symbols
    );
    // extraction must still be happening at all
    assert_has(&e.symbols, "class Public");
    assert_has(&e.symbols, "def Public.run(self) -> None");
}

#[test]
fn a_private_class_nested_in_a_public_class_is_hidden_with_its_methods() {
    let src = "class Outer:\n    class _Inner:\n        def run(self) -> None:\n            pass\n    class Open:\n        pass\n";
    let e = crate::extract("m.py", src);
    assert_eq!(e.symbols, vec!["class Outer", "class Outer.Open"]);
}

#[test]
fn python_two_classes_with_the_same_method_stay_distinct() {
    let src = "class A2:\n    def run(self) -> None:\n        pass\n\nclass B2:\n    def run(self) -> None:\n        pass\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "def A2.run(self) -> None");
    assert_has(&e.symbols, "def B2.run(self) -> None");
}

#[test]
fn exports_group_a_class_with_its_own_members() {
    let src = "@dataclass\nclass Article:\n    title: str\n\nclass Verdict:\n    decision: str\n";
    let e = crate::extract("models.py", src);
    assert_eq!(
        e.symbols,
        vec![
            "@dataclass class Article".to_string(),
            "Article.title: str".to_string(),
            "class Verdict".to_string(),
            "Verdict.decision: str".to_string(),
        ],
        "symbols: {:?}",
        e.symbols
    );
}

#[test]
fn python_function_local_definitions_are_not_module_exports() {
    let src = "def outer():\n    def nested():\n        pass\n    class LocalClass:\n        def m(self):\n            pass\n";
    let e = crate::extract("t.py", src);
    assert_eq!(
        e.symbols,
        vec!["def outer()".to_string()],
        "symbols: {:?}",
        e.symbols
    );
}

#[test]
fn python_functions_in_function_bodies_are_nested_items() {
    let src = "def outer():\n    @wraps(f)\n    def deco():\n        def deep():\n            pass\n    class Local:\n        def m(self):\n            pass\n\nclass A:\n    def meth(self):\n        def inner():\n            pass\n";
    let e = crate::extract("t.py", src);
    let nested: Vec<(&str, usize, usize, bool)> = e
        .items
        .iter()
        .map(|i| (i.name.as_str(), i.start_line, i.end_line, i.nested))
        .collect();
    assert_eq!(
        nested,
        [
            ("outer", 1, 8, false),
            ("deco", 2, 5, true),
            ("deep", 4, 5, true),
            ("A", 10, 13, false),
            ("meth", 11, 13, false),
            ("inner", 12, 13, true),
        ]
    );
    assert!(e
        .items
        .iter()
        .filter(|i| i.nested)
        .all(|i| !i.public && i.owner.is_none()));
}

#[test]
fn python_all_is_the_authoritative_export_gate() {
    let src = "__all__ = [\"Public\", \"shown\"]\n\nclass Public:\n    field: str\n    def m(self) -> None:\n        pass\n    def _hidden(self) -> None:\n        pass\n\nclass NotListed:\n    field: int\n\ndef shown() -> int:\n    return 1\n\ndef not_listed() -> int:\n    return 2\n\nHIDDEN_CONST = 5\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "class Public");
    assert_has(&e.symbols, "def Public.m(self) -> None");
    assert_has(&e.symbols, "def shown() -> int");
    assert_lacks(&e.symbols, "NotListed");
    assert_lacks(&e.symbols, "not_listed");
    assert_lacks(&e.symbols, "HIDDEN_CONST");
    // __all__ says nothing about what is public inside a class
    assert_lacks(&e.symbols, "_hidden");
}

#[test]
fn all_can_export_an_underscore_prefixed_free_name() {
    // `__all__` replaces the underscore convention for the *module-level*
    // name it lists — including a free item whose own name starts with
    // `_`. `__version__` is a real, common idiom; a re-application of
    // `name_filter` to the item's own name would silently override
    // `__all__` here instead of being replaced by it.
    let src = "__all__ = [\"_private_api\", \"__version__\", \"Public\"]\n\n__version__ = \"1.0\"\n\ndef _private_api() -> int:\n    return 1\n\nclass Public:\n    def _hidden(self) -> None:\n        pass\n";
    let e = crate::extract("m.py", src);
    // __all__ must be able to export a free underscore-prefixed name
    assert_has(&e.symbols, "def _private_api() -> int");
    assert_has(&e.symbols, "__version__ = \"1.0\"");
    assert_has(&e.symbols, "class Public");
    assert!(
        !e.symbols.iter().any(|s| s.contains("_hidden")),
        "a _hidden method of a listed class must stay hidden — the \
             convention always applies inside a class: {:?}",
        e.symbols
    );
}

#[test]
fn without_all_the_underscore_convention_still_hides_a_free_name() {
    // Counterpart of the test above: with no `__all__` at all, `exports`
    // is `None` and the convention must still apply to a free item's own name.
    let src =
        "def _private_api() -> int:\n    return 1\n\ndef public_api() -> int:\n    return 2\n";
    let e = crate::extract("m.py", src);
    assert_lacks(&e.symbols, "_private_api");
    assert_has(&e.symbols, "def public_api() -> int");
}

#[test]
fn a_computed_all_falls_back_to_the_underscore_convention() {
    let src = "__all__ = [\"a\"] + other.__all__\n\ndef a():\n    pass\n\ndef b():\n    pass\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "def a()");
    // a non-literal __all__ must not gate anything
    assert_has(&e.symbols, "def b()");
}

#[test]
fn an_all_containing_an_fstring_element_falls_back_to_the_underscore_convention() {
    let src = "x = 1\n__all__ = [f\"a{x}\", \"b\"]\n\ndef b():\n    pass\n\ndef c():\n    pass\n";
    let e = crate::extract("m.py", src);
    // an f-string element must invalidate __all__, falling back to the convention
    assert_has(&e.symbols, "def c()");
}

#[test]
fn a_second_module_level_all_assignment_wins() {
    let src = "__all__ = [\"a\"]\ndef a():\n    pass\ndef b():\n    pass\n__all__ = [\"b\"]\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "def b()");
    // the second, later __all__ must win over the first
    assert_lacks(&e.symbols, "def a()");
}

#[test]
fn an_augmented_all_after_a_literal_falls_back_to_the_underscore_convention() {
    // `__all__ += [...]` never matches the literal-assignment query shape
    // at all, so a prior literal assignment stayed visible and the
    // augmented one was invisible to `python_all` — the stale `["base_name"]`
    // kept governing and `added_name` was silently dropped even though it
    // is a real, file-defined, genuinely public name. The fix must see the
    // `+=` as the LAST statement touching `__all__` and fall back to the
    // convention for the whole module, exporting both names.
    let src = "__all__ = [\"base_name\"]\n__all__ += [\"added_name\"]\n\ndef base_name() -> int:\n    return 1\n\ndef added_name() -> int:\n    return 2\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "def base_name() -> int");
    // `__all__ +=` must fall back to the convention, not keep the stale literal
    assert_has(&e.symbols, "def added_name() -> int");
}

#[test]
fn a_recomputed_all_after_a_literal_falls_back_to_the_underscore_convention() {
    // `__all__ = compute()` after an earlier literal assignment: the
    // literal-only query saw only the first, literal assignment and never
    // noticed the module later recomputed its surface non-literally. The
    // recomputation must win as the last statement and fall back to the
    // convention.
    let src = "__all__ = [\"a\"]\ndef a():\n    pass\ndef b():\n    pass\n__all__ = compute()\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "def a()");
    // a non-literal reassignment must fall back to the convention
    assert_has(&e.symbols, "def b()");
}

#[test]
fn an_all_dot_extend_after_a_literal_falls_back_to_the_underscore_convention() {
    let src =
        "__all__ = [\"a\"]\n__all__.extend([\"b\"])\n\ndef a():\n    pass\n\ndef b():\n    pass\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "def a()");
    // __all__.extend(...) must fall back to the convention
    assert_has(&e.symbols, "def b()");
}

#[test]
fn python_assignments_keep_their_values_annotated_or_not() {
    let src = "MAX_IDS = 2000\nSUMMARY_LIMIT: int = 300\nPRISM_FILES: list[tuple[str, str]] = [(\"a\", \"b\")]\nAlias = list[int]\n_PRIVATE = 1\n";
    let e = crate::extract("state.py", src);
    assert_has(&e.symbols, "MAX_IDS = 2000");
    assert_has(&e.symbols, "SUMMARY_LIMIT: int = 300");
    assert_has(
        &e.symbols,
        "PRISM_FILES: list[tuple[str, str]] = [(\"a\", \"b\")]",
    );
    assert_has(&e.symbols, "Alias = list[int]");
    assert_lacks(&e.symbols, "_PRIVATE");
}

#[test]
fn python_class_fields_are_qualified_and_gated() {
    let src = "class Article:\n    title: str\n    url: str = \"\"\n    published: datetime | None = None\n    _hidden: int = 0\n";
    let e = crate::extract("models.py", src);
    assert_has(&e.symbols, "Article.title: str");
    assert_has(&e.symbols, "Article.url: str = \"\"");
    assert_has(&e.symbols, "Article.published: datetime | None = None");
    assert_lacks(&e.symbols, "_hidden");
}

#[test]
fn python_class_fields_are_emitted_once_not_twice() {
    let src = "class Article:\n    title: str\n";
    let e = crate::extract("m.py", src);
    // one pattern, one emission
    assert_eq!(e.symbols, vec!["class Article", "Article.title: str"]);
}

#[test]
fn python_long_values_are_truncated_on_a_character_boundary() {
    // Multibyte characters: a byte-indexed cut panics here.
    let src = format!("SYSTEM_PROMPT = \"{}\"\n", "naïve café — ".repeat(8));
    let e = crate::extract("judge.py", &src);
    // 48 characters of value, then the ellipsis
    assert_has(
        &e.symbols,
        "SYSTEM_PROMPT = \"naïve café — naïve café — naïve café — naïve ca…",
    );
}

#[test]
fn python_line_continuations_do_not_strand_the_strip_loop() {
    let src = "Z: int = \\\n    5\nX = \\\n    compute()\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "Z: int = 5");
    assert_has(&e.symbols, "X = compute()");
}

#[test]
fn crlf_line_continuations_do_not_strand_the_strip_loop() {
    // Under CRLF the continuation sequence is `\` `\r` `\n`, not `\` `\n`
    // — a plain `.replace("\\\n", " ")` never matches it, so `Z: int = \`
    // survives unfixed on a CRLF file, exactly the defect the join was
    // added to prevent. The CRLF form must be replaced first.
    let src = "Z: int = \\\r\n    5\r\nX = \\\r\n    compute()\r\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "Z: int = 5");
    assert_has(&e.symbols, "X = compute()");
}

#[test]
fn python_chained_assignment_yields_only_the_outer_name() {
    // Known loss, pinned so it is visible rather than surprising: `B` is
    // equally a public module-level name and is dropped — only `A` is
    // captured (the query anchors `assignment` as a direct child of
    // `expression_statement`; the nested `B = 2` sits under `right:`
    // instead). `A`'s value is kept whole, per the unannotated rule, and
    // that whole value happens to be the literal text `B = 2`.
    let src = "A = B = 2\n";
    let e = crate::extract("m.py", src);
    assert_eq!(e.symbols, vec!["A = B = 2".to_string()], "{:?}", e.symbols);
}

#[test]
fn a_comment_between_a_decorator_and_its_definition_is_dropped() {
    // The decorator span reaches the definition, so the comment sits
    // inside it and must be dropped by `render_span`.
    let src = "@dataclass  # noqa: D101\nclass Article:\n    title: str\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "@dataclass class Article");
    assert_lacks(&e.symbols, "noqa");
}

#[test]
fn a_comment_inside_a_retained_value_is_dropped() {
    // The value is rendered through the same path as the head, so a
    // comment inside it must be dropped the same way.
    let src = "CONFIG = {  # inline\n    \"a\": 1,\n}\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "CONFIG = { \"a\": 1, }");
    assert_lacks(&e.symbols, "inline");
}

#[test]
fn a_python_string_value_keeps_its_own_punctuation() {
    let src = "SEP = \" , \"\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "SEP = \" , \"");
}

#[test]
fn python_decorators_are_kept_with_their_arguments() {
    let src = "@dataclass(frozen=True)\nclass Article:\n    title: str\n";
    let e = crate::extract("models.py", src);
    assert_has(&e.symbols, "@dataclass(frozen=True) class Article");
    assert_has(&e.symbols, "Article.title: str");
}

#[test]
fn python_method_decorators_render_before_the_qualified_name() {
    let src = "class Article:\n    @property\n    def slug(self) -> str:\n        return \"\"\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "@property def Article.slug(self) -> str");
}

#[test]
fn python_stacked_decorators_all_survive() {
    let src = "@a\n@b(1)\n@c.d.e\ndef stacked() -> int:\n    return 0\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "@a @b(1) @c.d.e def stacked() -> int");
}

#[test]
fn python_conditionally_defined_module_functions_are_kept() {
    let src = "if TYPE_CHECKING:\n    def type_only():\n        pass\n\ntry:\n    def optional_dep():\n        pass\nexcept ImportError:\n    pass\n";
    let e = crate::extract("t.py", src);
    assert_has(&e.symbols, "def type_only()");
    assert_has(&e.symbols, "def optional_dep()");
}

#[test]
fn equal_signatures_are_listed_once() {
    let src = "if NEW:\n    def parse(text: str) -> int:\n        return 1\nelse:\n    def parse(text: str) -> int:\n        return 2\n";
    let e = crate::extract("m.py", src);
    assert_eq!(e.symbols, vec!["def parse(text: str) -> int"]);
}

#[test]
fn python_unannotated_assignment_renders_its_value_once() {
    // `signature_cut` leaves an unannotated value to the join, so the
    // head must not also carry it: `MAX_IDS = 2000 = 2000` is the failure.
    let src = "MAX_IDS = 2000\n";
    let e = crate::extract("state.py", src);
    assert_eq!(e.symbols, vec!["MAX_IDS = 2000"]);
}

#[test]
fn python_assignment_without_spaces_is_normalized_and_budgeted() {
    // The ` = ` is emitted by the join now, never copied from source, so a
    // no-space assignment renders like every sibling and the budget
    // applies however long the value is.
    let long = "y".repeat(200);
    let src = format!("CACHE_DIR=\"/var/cache/store\"\nBIG=\"{long}\"\n");
    let e = crate::extract("state.py", &src);
    assert_has(&e.symbols, "CACHE_DIR = \"/var/cache/store\"");
    assert_has(&e.symbols, &format!("BIG = \"{}…", "y".repeat(47)));
}

#[test]
fn a_value_of_exactly_48_characters_is_kept_whole_and_one_more_is_cut() {
    // Characters, not bytes: 46 `é` and two quotes are 48 characters, 94 bytes.
    let at = format!("X = \"{}\"", "é".repeat(46));
    let e = crate::extract("m.py", &format!("{at}\n"));
    assert_eq!(e.symbols, vec![at]);
    let e = crate::extract("m.py", &format!("X = \"{}\"\n", "é".repeat(47)));
    assert_eq!(e.symbols, vec![format!("X = \"{}…", "é".repeat(47))]);
}

#[test]
fn python_annotated_over_budget_omits_the_value() {
    // An annotation survives to carry the contract, so omit rather than
    // truncate — the same rule Rust follows, for the same reason.
    let long = "z".repeat(200);
    let src = format!("BIG: str = \"{long}\"\n");
    let e = crate::extract("state.py", &src);
    // annotated must omit, not truncate
    assert_eq!(e.symbols, vec!["BIG: str"]);
}

#[test]
fn a_continuation_inside_the_value_is_joined() {
    // The continuation is a child of the `binary_operator` that IS the
    // `right` field — it is inside the value, not between `=` and the
    // value. Without the shared pipeline this renders `X = 1 + \ 2`.
    let src = "X = 1 + \\\n    2\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "X = 1 + 2");
}

#[test]
fn a_crlf_continuation_inside_the_value_is_joined() {
    let src = "X = 1 + \\\r\n    2\r\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "X = 1 + 2");
}

#[test]
fn a_wrapped_call_value_keeps_its_punctuation_tidied() {
    // Without the shared pipeline this renders `Y = compute( 1, 2, )`.
    let src = "Y = compute(\n    1,\n    2,\n)\n";
    let e = crate::extract("m.py", src);
    assert_has(&e.symbols, "Y = compute(1, 2)");
}

#[test]
fn a_const_only_module_does_not_summarize_as_a_truncated_value() {
    // `pick_summary_fallback` reaches FreeValue when a module has no
    // FreeDef and no docstring, and summaries are scored at W_SUMMARY=1.5
    // in search — higher than body text. A truncated fragment must never
    // land there.
    let long = "q".repeat(200);
    let src = format!("TOPICS: list[str] = [\"{long}\"]\n");
    let e = crate::extract("settings.py", &src);
    assert_eq!(e.summary.as_deref(), Some("TOPICS: list[str]"));
}

#[test]
fn python_items_cover_classes_private_methods_functions_and_constants() {
    let src = "@dataclass\nclass Article:\n    \"\"\"A news article.\n\n    More.\"\"\"\n    title: str = \"\"\n\n    def _slug(self):\n        \"\"\"Slug for the url.\"\"\"\n        return 1\n\n\ndef _helper():\n    pass\n\nLIMIT = 5\n";
    let e = crate::extract("a.py", src);
    assert_eq!(
        item_lines(&e.items),
        vec![
            "1-10 type Article pub",
            "8-10 method Article::_slug priv",
            "13-14 fn _helper priv",
            "16-16 value LIMIT pub",
        ]
    );
    assert_eq!(e.items[0].doc.as_deref(), Some("A news article."));
    assert_eq!(e.items[1].doc.as_deref(), Some("Slug for the url."));
}

#[test]
fn python_hash_comment_is_a_doc() {
    let src = "x = 1\n# Wait longer than w_min counts as a stall.\nW_MIN_DEFAULT_S = 5\n";
    assert_eq!(
        doc_of("a.py", src, "W_MIN_DEFAULT_S"),
        (Some("Wait longer than w_min counts as a stall.".into()), 2)
    );
}

#[test]
fn a_multi_line_hash_doc_extends_the_range_over_every_comment_line() {
    let src = "x = 1\n# Builds the frame\n# for each caller.\ndef f():\n    pass\n";
    let e = crate::extract("a.py", src);
    let f = e.items.iter().find(|i| i.name == "f").unwrap();
    assert_eq!(
        (f.doc.as_deref(), f.start_line, f.end_line),
        (Some("Builds the frame for each caller."), 2, 5)
    );
}

#[test]
fn a_blank_line_between_a_comment_and_its_item_leaves_the_item_undocumented() {
    let src = "x = 1\n# Stray note about nothing.\n\ndef f():\n    pass\n# Builds the frame for each caller.\ndef g():\n    pass\n";
    assert_eq!(doc_of("a.py", src, "f"), (None, 4));
    assert_eq!(
        doc_of("a.py", src, "g"),
        (Some("Builds the frame for each caller.".into()), 6)
    );
}

#[test]
fn a_sphinx_attribute_comment_drops_its_colon() {
    let src = "x = 1\n#: Sphinx attribute doc.\nLIMIT = 5\n";
    assert_eq!(
        doc_of("a.py", src, "LIMIT"),
        (Some("Sphinx attribute doc.".into()), 2)
    );
}

#[test]
fn python_docstring_wins_over_hash_comment() {
    let src = "x = 1\n# note\ndef f():\n    \"\"\"Real doc.\"\"\"\n";
    assert_eq!(doc_of("a.py", src, "f"), (Some("Real doc.".into()), 3));
    let src = "x = 1\n# Builds the frame for each caller.\ndef f():\n    \"\"\"Real doc.\"\"\"\n";
    assert_eq!(doc_of("a.py", src, "f"), (Some("Real doc.".into()), 3));
}
