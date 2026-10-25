//! Tree-sitter code extraction: per-language specs, the process-wide query cache, and the signature pipeline behind `## Exports` and `## Imports`.

use crate::formats::{summarize, Extractor};
use crate::model::{
    basename, one_line, slugify, title_from_slug, CallSite, CallVia, CodeItem, CodeItemKind,
    Entity, SourceKind,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;
use tree_sitter::{
    Language, Node, Parser, Query, QueryCursor, QueryMatch, StreamingIterator, Tree,
};

pub struct CodeExtractor;

pub(crate) struct LangSpec {
    pub(crate) lang_name: &'static str,
    pub(crate) language: Language,
    /// Query capturing:
    ///   @def    - the definition node whose text is shortened to a signature
    ///   @name   - the definition's name (used for post-hoc export/visibility filtering)
    ///   @import - a module path node
    ///   @vis    - the visibility-modifier text that `vis_filter` gates on
    ///   @item   - a definition to list as an item (with its @name); an
    ///             @item match never contributes to exports, and a match
    ///             carries @def or @item, never both
    pub(crate) query_src: &'static str,
    /// Keep a captured definition only if its @name text passes this filter.
    /// Used where the grammar can't express the gate structurally (Python's
    /// leading-underscore convention, Go's capitalized-identifier convention).
    pub(crate) name_filter: fn(&str) -> bool,
    /// Applied to the text of a `@vis` capture. A pattern that captures no
    /// `@vis` is not gated at all — items inside a trait impl or a trait
    /// declaration carry no visibility modifier yet are public through the
    /// trait.
    pub(crate) vis_filter: fn(&str) -> bool,
    /// Trailing characters to strip off a built signature (e.g. Rust's `;`
    /// on a body-less unit/tuple struct, Python's `:` after the parameter list).
    pub(crate) strip_trailing: &'static [char],
    /// Rejects a definition, or reports the scopes enclosing it. Per-language
    /// because Rust guards module level with an allow-list of item containers
    /// while Python denies exactly one (a function body).
    pub(crate) placement: fn(Node, &str) -> Placement,
    /// Separator between owner and member name in a qualified signature.
    pub(crate) owner_sep: &'static str,
    /// The module's own declaration of its public surface, when the language
    /// has one and the module states it literally. `None` means fall back to
    /// `name_filter`.
    pub(crate) export_set: fn(&Tree, &str) -> Option<BTreeSet<String>>,
    /// Join Python's explicit `\`-newline line continuations before
    /// collapsing. Not shared: JS and TS allow the same sequence inside a
    /// string literal, and a default-parameter value is part of the retained
    /// signature span, so stripping it there would silently rewrite their
    /// signatures.
    pub(crate) join_continuations: bool,
    /// The node whose start byte opens the signature. Python returns the
    /// enclosing `decorated_definition`; every other language returns the
    /// definition itself.
    pub(crate) sig_start: fn(Node) -> Node,
    /// The node whose start byte a resolved owner is spliced in at, given the
    /// `@name` node. Identity for every language whose declarations name
    /// themselves unqualified, so the owner is inserted directly ahead of the
    /// name (`pub fn ` + `Wiki::` + `search(&self)`).
    ///
    /// C++ is the exception, and the reason this is a seam at all: an
    /// out-of-line definition writes its own qualifier in the source, so
    /// anchoring at the name would insert the owner ahead of a qualifier
    /// already there — `void OrderBook::OrderBook::apply()`. Anchoring at the
    /// outermost `qualified_identifier` makes the owner REPLACE that prefix,
    /// which is also what strips a namespace: `void trading::OrderBook::apply()`
    /// and the in-class declaration of the same method render one identical
    /// signature.
    pub(crate) owner_anchor: fn(Node) -> Node,
    /// The group a `@def` with no `@name` shares with its own members — only
    /// Rust's trait-impl header pattern needs this; every other language
    /// resolves a group from `owner` or `@name` and never reaches it.
    pub(crate) header_group: fn(Node, &str) -> Option<String>,
    /// What a named sibling directly above an item is to that item: its doc
    /// comment, an attribute that belongs to it, a plain comment that may
    /// document it (C++, JS/TS, Python), or neither. Rust and Go read only
    /// their own doc convention, so a plain comment stops their walk.
    pub(crate) item_lead: fn(Node, &str) -> Lead,
    /// An item's doc when it lives inside the definition (Python's
    /// docstring) rather than above it. Consulted only when no `Doc` lead
    /// was attached, and before a plain comment block.
    pub(crate) body_doc: fn(Node, &str) -> Option<String>,
    /// True when the query's @def patterns already gate visibility, so an
    /// @item-only match is private by construction (Rust: a public item
    /// also matches a @def pattern, and that match's flag wins). False
    /// makes an @item-only match run `should_keep` itself.
    pub(crate) query_gates_visibility: bool,
    /// What a captured definition is and where its signature stops. Vue and
    /// Svelte inherit their script language's hook.
    pub(crate) shape: fn(Node) -> Shape,
}

impl LangSpec {
    /// A spec with every hook at its no-op default: a language lists only
    /// what it overrides and spreads `..LangSpec::base(..)` for the rest.
    pub(crate) fn base(
        lang_name: &'static str,
        language: Language,
        query_src: &'static str,
    ) -> Self {
        LangSpec {
            lang_name,
            language,
            query_src,
            name_filter: keep_all,
            vis_filter: keep_any_vis,
            strip_trailing: &[],
            placement: |_, _| Placement::Scoped(Vec::new()),
            owner_sep: ".",
            export_set: no_export_set,
            join_continuations: false,
            sig_start: sig_start_identity,
            owner_anchor: owner_anchor_identity,
            header_group: no_header_group,
            item_lead: |_, _| Lead::Stop,
            body_doc: no_body_doc,
            query_gates_visibility: false,
            shape: default_shape,
        }
    }
}

/// See `LangSpec::item_lead`.
pub(crate) enum Lead {
    /// A doc-comment line, its marker stripped.
    Doc(String),
    /// A plain comment's text, markers stripped, lines kept: the item's doc
    /// only as a fallback, and only if `plain_doc` takes it.
    Plain(String),
    /// An attribute or decorator: part of the item's range, not its doc.
    Attr,
    /// Not part of the item; the walk ends here.
    Stop,
}

/// The text of a `//` or `/* … */` comment, markers and a block's leading
/// `*` stripped, lines kept.
pub(crate) fn plain_comment_text(comment: &str) -> String {
    if let Some(line) = comment.strip_prefix("//") {
        return line.trim().to_string();
    }
    let body = comment.trim_start_matches("/*").trim_end_matches("*/");
    let lines: Vec<&str> = body
        .lines()
        .map(|l| {
            let l = l.trim();
            // A run of stars is a divider (`/*****`), not an empty line.
            if l.len() > 1 && l.chars().all(|c| c == '*') {
                l
            } else {
                l.trim_start_matches('*').trim()
            }
        })
        .collect();
    lines.join("\n")
}

/// The doc a plain comment block directly above `item_name` gives, or `None`
/// when the block is not a doc: a file header, a banner or divider, a group
/// label ("Getters"), a TODO, or commented-out code.
pub(crate) fn plain_doc(
    lines: &[String],
    item_name: &str,
    starts_at_file_top: bool,
) -> Option<String> {
    if starts_at_file_top
        || lines
            .iter()
            .any(|l| l.contains("SPDX-License-Identifier") || l.contains("Copyright"))
    {
        return None;
    }
    // Divider lines (no letters) go, and so does a title framed on its own
    // line (`--- Risk ---`, `─── Risk ───`): one opening and closing with a
    // run of three or more of the same character that is neither
    // alphanumeric nor whitespace. A `===` inside prose stays. Empty lines
    // stay as paragraph breaks.
    let opens_with_run = |mut cs: std::str::Chars| {
        cs.next().is_some_and(|c| {
            !c.is_alphanumeric()
                && !c.is_whitespace()
                && cs.take(2).filter(|x| *x == c).count() == 2
        })
    };
    let framed = |l: &str| {
        opens_with_run(l.chars()) && opens_with_run(l.chars().rev().collect::<String>().chars())
    };
    let lines: Vec<&str> = lines.iter().map(|l| l.trim()).collect();
    let divider = lines
        .iter()
        .any(|l| !l.is_empty() && !l.chars().any(char::is_alphabetic));
    let mut lines: Vec<&str> = lines
        .into_iter()
        .filter(|l| l.is_empty() || l.chars().any(char::is_alphabetic) && !framed(l))
        .collect();
    while lines.first() == Some(&"") {
        lines.remove(0);
    }
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines.dedup_by(|a, b| a.is_empty() && b.is_empty());
    let text = lines.join("\n");
    let lines: Vec<&str> = lines.into_iter().filter(|l| !l.is_empty()).collect();
    // A lone line between dividers is a section banner's title; a block
    // opening with TODO (a wrapped one included) is a note, not a doc.
    if lines.is_empty()
        || divider && lines.len() == 1
        || ["TODO", "FIXME", "XXX", "HACK"]
            .iter()
            .any(|t| lines[0].starts_with(t))
        || 2 * lines
            .iter()
            .filter(|l| l.ends_with([';', '{', '}']))
            .count()
            >= lines.len()
    {
        return None;
    }
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .flat_map(crate::query::Wiki::name_words)
            .filter(|w| w.chars().count() >= 3)
            .collect()
    };
    let label = lines.len() == 1
        && lines[0].split_whitespace().count() <= 3
        && !lines[0].ends_with(['.', '!', '?', ':'])
        && {
            let name = words(item_name);
            !words(lines[0]).iter().any(|w| name.contains(w))
        };
    (!label).then_some(text)
}

/// `body_doc` for a language with no in-body doc convention.
fn no_body_doc(_: Node, _: &str) -> Option<String> {
    None
}

fn sig_start_identity(def: Node) -> Node {
    def
}

/// `owner_anchor` for a language whose declarations never write their own
/// qualifier: the owner is spliced in at the name node itself.
fn owner_anchor_identity(name: Node) -> Node {
    name
}

fn keep_all(_name: &str) -> bool {
    true
}

fn keep_any_vis(_vis: &str) -> bool {
    true
}

fn no_export_set(_tree: &Tree, _text: &str) -> Option<BTreeSet<String>> {
    None
}

/// Only Rust emits a `@def` without a `@name` (the trait-impl header pattern),
/// so every other language never reaches this.
fn no_header_group(_def: Node, _text: &str) -> Option<String> {
    None
}

/// Where a captured definition sits relative to the module's public surface.
///
/// Replaces the previous `owner_of` + `def_filter` pair. The two were split
/// only because Rust could answer them independently; Python answers both from
/// a single upward traversal of the tree, and keeping them apart would
/// traverse it twice and duplicate the privacy check.
pub(crate) enum Placement {
    /// Not part of the module's surface — drop the definition outright.
    Rejected,
    /// Enclosing scope names, outermost first. Empty means a free item whose
    /// signature is used verbatim.
    Scoped(Vec<String>),
}

fn lang_for_ext(ext: &str) -> Option<LangSpec> {
    Some(match ext {
        "rs" => super::extract_rust::rust_spec(),
        "py" => super::extract_python::python_spec(),
        "js" => super::extract_simple::js_spec(),
        "ts" => super::extract_simple::ts_spec(),
        "jsx" => super::extract_jsx::jsx_spec(),
        "tsx" => super::extract_jsx::tsx_spec(),
        "vue" => super::extract_vue::vue_spec(),
        "svelte" => super::extract_svelte::svelte_spec(),
        "go" => super::extract_simple::go_spec(),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "h" => super::extract_cpp::cpp_spec(),
        _ => return None,
    })
}

/// The lowercased extension of `rel_path`, or `""` when the basename has no
/// extension (`Makefile`) or is a dotfile (`.gitignore`).
///
/// `Registry::ext_of` lowercases when dispatching, but `CodeExtractor::extract`
/// used to recompute the extension without lowercasing, so `Book.H` reached
/// this extractor and then fell through `lang_for_ext` to zero symbols.
pub(crate) fn ext_of(rel_path: &str) -> String {
    let base = basename(rel_path);
    match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}

/// A source file's claim to the shorter name when two files share a page name.
/// Lower wins.
///
/// A C header declares a component's public surface and is what `#include`
/// names, so it outranks a C++-only header, which outranks the translation
/// unit implementing either. A TOTAL order, not a binary: with every header at
/// one rank, adding `foo.hpp` beside `foo.h` would tie at the top, dissolve
/// the strict win in `disambiguate_ids`, and rename the pre-existing page.
pub(crate) fn source_rank(source_path: &str) -> u8 {
    match ext_of(source_path).as_str() {
        "h" => 0,
        "hpp" | "hh" | "hxx" => 1,
        _ => 2,
    }
}

/// Every extension `CodeExtractor` claims. Shared by `extensions()` and by
/// `QUERIES` so the registry can never miss a language the extractor accepts —
/// a missing entry would strip that language of all symbols and imports.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "vue", "svelte", "go", "cpp", "cc", "cxx", "hpp", "hh",
    "hxx", "h",
];

/// Each language's query, compiled once per process instead of once per file.
///
/// Compilation is a flat ~1.3ms and does not depend on the file, so doing it
/// per file dominated extraction for ordinary source sizes (88% of tree-sitter
/// work at 2KB, 43% at 20KB). Matches the `LazyLock<Regex>` statics in
/// `graph`, `lint`, `rewrite`, and `text`.
///
/// Panicking here is deliberate and mirrors those `Regex::new(...).unwrap()`
/// sites: the query strings are compile-time constants, so a failure is a
/// programming error, not a runtime condition. `validate_queries` forces this
/// before any output is written, so the panic cannot arrive mid-compile with
/// pages already on disk.
static QUERIES: LazyLock<BTreeMap<&'static str, Query>> = LazyLock::new(|| {
    CODE_EXTENSIONS
        .iter()
        .map(|ext| {
            let spec = lang_for_ext(ext)
                .unwrap_or_else(|| panic!("no LangSpec registered for extension {ext:?}"));
            let query = Query::new(&spec.language, spec.query_src).unwrap_or_else(|e| {
                panic!("invalid tree-sitter query for {}: {e:?}", spec.lang_name)
            });
            (*ext, query)
        })
        .collect()
});

/// Force every language query to compile.
///
/// Called at the top of a compile so a malformed query fails loudly and
/// deterministically *before* any page is written. Without this the first
/// failure would surface lazily inside a rayon worker, partway through a
/// parallel run, with output already committed for other files.
pub(crate) fn validate_queries() {
    LazyLock::force(&QUERIES);
    super::extract_python::validate_queries();
    super::extract_rust::validate_queries();
}

impl Extractor for CodeExtractor {
    fn extensions(&self) -> &[&str] {
        CODE_EXTENSIONS
    }

    fn extract(&self, rel_path: &str, text: &str) -> Entity {
        let ext = ext_of(rel_path);
        let ext = ext.as_str();

        // Test modules are orientation noise that inflate the page body, its
        // token_estimate, and the neighbors budget (dogfood finding #12) —
        // splice them out, leaving an honest omission marker. Stripping runs
        // BEFORE extraction (finding #13) so the body, the symbols, the
        // imports, and the doc comment all describe the same source.
        // When stripping finds nothing it hands back the tree it already
        // parsed, and extraction reuses it rather than parsing the same bytes
        // again. A file that IS stripped must reparse: the text changed.
        // Items are the exception to "extract the stripped text": their lines
        // must be the original file's, and a test module is code an agent
        // reads too, so they come from the original text and its tree.
        let (source, pre_parsed, original, test_spans) = match (ext == "rs")
            .then(|| super::extract_rust::strip_rust_test_modules(text))
            .flatten()
        {
            Some(super::extract_rust::Stripped::Unchanged(tree)) => {
                (text.to_string(), Some(tree), None, Vec::new())
            }
            Some(super::extract_rust::Stripped::Rewritten(out, tree, spans)) => {
                (out, None, Some(tree), spans)
            }
            None => (text.to_string(), None, None, Vec::new()),
        };

        // A single-file component is markup around code: only its `<script>`
        // blocks are extracted, under the spec their `lang` names. The view
        // blanks everything else in place, so every byte offset still points
        // into `source` and the rest of the pipeline needs no change.
        let script = match ext {
            "vue" => Some(super::extract_vue::vue_script(&source)),
            "svelte" => Some(super::extract_svelte::svelte_script(&source)),
            _ => None,
        };
        let (code_ext, code) = script
            .as_ref()
            .map_or((ext, source.as_str()), |v| (v.ext, v.text.as_str()));

        let info = extract_code(code_ext, code, pre_parsed).unwrap_or_else(|| CodeInfo {
            lang: ext.to_string(),
            ..CodeInfo::default()
        });
        // The original pass gates `public` on the unstripped tree, where a
        // bare `pub fn` inside the test module passes too. Exports come from
        // the stripped text, so an item is public only if the stripped pass
        // has the same item as public.
        let mut items = match original {
            Some(tree) => {
                let mut items = extract_code(ext, text, Some(tree))
                    .map(|i| i.items)
                    .unwrap_or_default();
                for item in &mut items {
                    item.public &= info.items.iter().any(|s| {
                        s.public
                            && s.name == item.name
                            && s.owner == item.owner
                            && s.signature == item.signature
                    });
                }
                items
            }
            None => info.items,
        };
        let test_file = crate::model::is_test_path(rel_path);
        for item in &mut items {
            item.test = test_file
                || test_spans
                    .iter()
                    .any(|&(s, e)| s <= item.start_line && item.end_line <= e)
                || (ext == "rs"
                    && matches!(item.kind, CodeItemKind::Fn | CodeItemKind::Method)
                    && super::extract_rust::has_test_attribute(text, item.start_line));
        }
        let mut lang_name = info.lang;

        // A component is reported as its own format, whichever language its
        // script is written in.
        if script.is_some() {
            if let Some(spec) = lang_for_ext(ext) {
                lang_name = spec.lang_name.to_string();
            }
        }

        let docstring = info.docstring.or_else(|| leading_doc(code, code_ext));
        // `body` is deliberately NOT scanned for a summary: source code is not
        // prose, so letting summarize() hunt line-by-line for a "real
        // sentence" produces garbage. Only a real docstring, or (failing
        // that) an exported signature, is an acceptable summary.
        let summary = summarize(
            None,
            docstring.as_deref(),
            "",
            info.summary_fallback.as_deref(),
        );
        let name = derive_code_name(rel_path);

        Entity {
            id: slugify(&name),
            name,
            body: source,
            kind: SourceKind::Code { lang: lang_name },
            summary,
            symbols: info.symbols,
            imports: info.imports,
            defined: info.defined,
            methods: info.methods,
            items,
            ..Entity::default()
        }
    }
}

/// What one `extract_code` pass yields. The summary fallback is chosen here
/// rather than by the caller: freeness is known only while the captures are
/// in hand, and `symbols` alone cannot be reinterpreted after sorting.
#[derive(Default)]
struct CodeInfo {
    lang: String,
    symbols: Vec<String>,
    imports: Vec<String>,
    defined: Vec<String>,
    methods: Vec<String>,
    items: Vec<CodeItem>,
    docstring: Option<String>,
    summary_fallback: Option<String>,
}

/// The shape a captured definition has, recorded at the point of capture
/// rather than recovered later from its rendered signature text. The summary
/// fallback needs to prefer a free item, then an `impl` header, before
/// falling back to whatever sorts first — and a string sniff on the rendered
/// signature is the wrong way to tell them apart: a generic trait impl
/// renders as `impl<T: Clone> Display for Wrapper<T>`, which does not start
/// with `impl ` (it starts with `impl<`), and `unsafe impl Send for Foo`
/// starts with neither. `build_signature` already splices the owner in at a
/// byte offset specifically so there is no substring search to mismatch;
/// re-deriving "is this an impl header" from text would reintroduce exactly
/// that kind of mismatch. The two `Option`s already in hand at the push site
/// determine the kind exactly, so it is captured there instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ItemKind {
    /// A top-level `class`/`def`/`struct`/`fn` — the best summary fallback.
    FreeDef,
    /// A trait-impl header (a `@def` with no `@name`).
    Header,
    /// A top-level value binding: Python `assignment`, Rust
    /// `const_item`/`static_item`/`type_item`. Uppercase constants sort ahead
    /// of lowercase `class`/`def` under plain lexicographic order, so without
    /// this rank they would take over the summary of every module that
    /// declares one.
    FreeValue,
    /// A `pub mod` declaration. Ranked below every real definition: a module
    /// list says what the file *contains*, not what it *is*, and as a bare
    /// signature `pub mod a` sorts ahead of `pub struct Store` (`m` < `s`),
    /// so without its own rank a module would take over the summary of every
    /// file that declares one — `lib.rs`, which declares nothing else, first.
    Module,
    /// A method, field, or associated item.
    Member,
}

/// What a definition is, as far as its own grammar is concerned.
///
/// Only the shape's *own language* can answer this — `Rank::Value` covers
/// Rust's `const`/`static`/`type` and Python's `assignment`, node kinds that
/// have nothing to do with each other. Consulted only for items `placement`
/// already reported as free: an associated `const` inside a trait impl is a
/// `Member`, not a `FreeValue`, and `classify` enforces that precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rank {
    /// A callable or type declaration — `fn`, `struct`, `class`, `def`.
    Def,
    /// A name bound to a value.
    Value,
    /// A module declaration.
    Module,
}

/// The grammar-derived facts about one definition node that the shared core
/// cannot know: what it is, where its signature stops, and what value (if any)
/// it re-appends.
///
/// This is the seam that keeps Rust and Python node kinds out of this file.
/// It follows the same consolidation `Placement` made — one hook answering
/// several questions from one look at the node, rather than several switches
/// each re-deriving the node's kind.
pub(crate) struct Shape<'a> {
    pub(crate) rank: Rank,
    /// Where the signature stops. `None` renders the whole node.
    pub(crate) cut: Option<Node<'a>>,
    /// A value to re-append after the head, subject to `VALUE_BUDGET`.
    ///
    /// Deliberately independent of `rank`: a Rust type alias is `Rank::Value`
    /// with `value: None`, because it names its target in a `type` field, not
    /// a `value` field, and already renders whole. Deriving one from the other
    /// — `rank == Value` implying `value.is_some()` or the reverse — breaks
    /// type aliases. `rust_module_level_const_static_and_type_alias` catches it.
    pub(crate) value: Option<Node<'a>>,
    /// Whether the head still states a type once the value is dropped. Decides
    /// omit-vs-truncate when the value is over budget. See `append_value`.
    pub(crate) has_type: bool,
}

/// The signature cut every language shares: stop at the body, at a wrapped
/// declaration's body, or at a value.
///
/// Written as three independent lookups rather than a chain of `?`: a Rust
/// `const_item` has no `declaration` field, so an early return there would
/// skip the `value` lookup and leave the initializer in the signature.
pub(crate) fn default_cut(def: Node) -> Option<Node> {
    if let Some(body) = def.child_by_field_name("body") {
        return Some(body);
    }
    if let Some(declaration) = def.child_by_field_name("declaration") {
        if let Some(body) = declaration.child_by_field_name("body") {
            return Some(body);
        }
    }
    def.child_by_field_name("value")
}

/// A plain definition: cut at the shared boundary, retain no value.
///
/// `has_type: true` is the safe default — it means an over-budget value is
/// omitted rather than truncated. Languages with no retained value never
/// consult it.
pub(crate) fn default_shape(def: Node) -> Shape {
    Shape {
        rank: Rank::Def,
        cut: default_cut(def),
        value: None,
        has_type: true,
    }
}

/// Capture indices for one language's query, resolved once per extraction.
struct CaptureIdx {
    def: Option<u32>,
    name: Option<u32>,
    vis: Option<u32>,
    imp: Option<u32>,
    item: Option<u32>,
}

/// The single-valued captures one query match can contribute: the
/// definition (or item) and name nodes, plus the visibility keyword's text.
/// Imports are deliberately absent: a single match may carry several, so they
/// go straight into the caller's list rather than through this struct.
struct MatchParts<'a> {
    def: Option<Node<'a>>,
    name: Option<Node<'a>>,
    vis: Option<&'a str>,
    item: Option<Node<'a>>,
}

/// Sort one match's captures into the def/name/vis slots, appending any
/// import capture to `imports` in encounter order.
fn split_captures<'a>(
    m: &QueryMatch<'a, 'a>,
    idx: &CaptureIdx,
    text: &'a str,
    imports: &mut Vec<String>,
) -> MatchParts<'a> {
    let mut parts = MatchParts {
        def: None,
        name: None,
        vis: None,
        item: None,
    };
    for cap in m.captures() {
        if Some(cap.index) == idx.def {
            parts.def = Some(cap.node);
        } else if Some(cap.index) == idx.name {
            parts.name = Some(cap.node);
        } else if Some(cap.index) == idx.vis {
            parts.vis = text.get(cap.node.byte_range());
        } else if Some(cap.index) == idx.item {
            parts.item = Some(cap.node);
        } else if Some(cap.index) == idx.imp {
            if let Some(raw) = text.get(cap.node.byte_range()) {
                imports.push(raw.trim().trim_matches(['"', '\'']).to_string());
            }
        }
    }
    parts
}

/// Four gates, all of which must pass. In the order of the returned
/// expression: `root_ok` (the module-level name), `own_name_ok` (the item's
/// own name), the intervening scopes in `chain`, and `vis`.
///
/// `__all__`, when the module declares one literally, replaces the
/// convention for the *module-level* name — the item itself when free, the
/// outermost enclosing class when a member. The convention always applies
/// inside a class, because `__all__` says nothing about what is public within
/// one.
///
/// For a free item (`chain` empty), `gate_root` IS `name_text`, so `root_ok`
/// has already decided this exact name against `exports` when the module
/// declares one. Re-applying `name_filter` to it here would let the underscore
/// convention override `__all__` instead of being replaced by it —
/// `__all__ = ["__version__"]` would otherwise never export `__version__`. A
/// member's own name is not `gate_root` (the outermost enclosing class is), so
/// the convention must still gate it unconditionally.
///
/// The third gate covers the scopes *between* the outermost one and the item
/// itself: `chain.iter().skip(1)`, skipping the first because
/// `chain.first()` is already `gate_root`. Every intervening scope name must
/// pass `name_filter`, so a public method on a private inner class stays out.
///
/// The fourth is `vis`: when the language's query captured a visibility
/// keyword for this definition, it must pass `spec.vis_filter` (e.g.
/// rejecting Rust's private-by-default items). Absent a `vis` capture the
/// gate passes, which is what lets trait-impl items through — they carry no
/// visibility modifier yet are public through the trait.
fn should_keep(
    vis: Option<&str>,
    chain: &[String],
    name_text: Option<&str>,
    exports: Option<&BTreeSet<String>>,
    spec: &LangSpec,
) -> bool {
    let gate_root = chain.first().map(String::as_str).or(name_text);
    let root_ok = match (exports, gate_root) {
        (Some(set), Some(root)) => set.contains(root),
        (_, Some(root)) => (spec.name_filter)(root),
        (_, None) => true,
    };
    let own_name_ok = if exports.is_some() && chain.is_empty() {
        true
    } else {
        name_text.is_none_or(|n| (spec.name_filter)(n))
    };
    root_ok
        && own_name_ok
        && chain.iter().skip(1).all(|c| (spec.name_filter)(c))
        && vis.is_none_or(|v| (spec.vis_filter)(v))
}

/// The shape of a kept definition, decided from the two `Option`s already in
/// hand plus the node's own kind, rather than sniffed back out of the
/// rendered signature.
/// Precedence is Header > Member > `rank`: an associated `const` inside a
/// trait impl carries `Rank::Value` from the grammar but is a `Member`.
///
/// Mutation-tested, and the honest result is that no current output
/// distinguishes the two. A `const`/`static`/`type` member always renders a
/// signature that sorts ahead of a `fn` member's (same `pub ` prefix, then
/// `c`/`s`/`t` before `f`), so the group sort and `pick_summary_fallback`'s
/// min-signature both land identically whether such an item is `Member` or
/// `FreeValue`. Swapping the precedence here breaks no test — not because the
/// suite is weak, but because the difference is currently unobservable.
///
/// Keep the order anyway. It stops being unobservable the moment
/// `pick_summary_fallback`'s rungs or `ItemKind`'s derived `Ord` change, and a
/// `Member` reported as a free item is wrong on its face regardless of whether
/// today's sort happens to hide it.
fn classify(name_node: Option<Node>, owner: Option<&str>, rank: Rank) -> ItemKind {
    if name_node.is_none() {
        ItemKind::Header
    } else if owner.is_some() {
        ItemKind::Member
    } else {
        match rank {
            Rank::Module => ItemKind::Module,
            Rank::Value => ItemKind::FreeValue,
            Rank::Def => ItemKind::FreeDef,
        }
    }
}

/// One kept definition: `(group, kind, name, signature)`. See the sort
/// comment at `collected`'s declaration below for what each field means.
type Collected = (String, ItemKind, String, String);

/// The `group` component of the sort key — the field that places a definition
/// next to its own members: `class Article` and `Article.title: str` both
/// group under `Article`.
fn group_key(
    owner: Option<&str>,
    name_text: Option<&str>,
    def: Node,
    text: &str,
    spec: &LangSpec,
) -> String {
    match (owner, name_text) {
        (Some(o), _) => o.to_string(),
        (None, Some(n)) => n.to_string(),
        (None, None) => (spec.header_group)(def, text).unwrap_or_default(),
    }
}

/// The module's summary when no docstring supplies one.
///
/// This must not read `collected`'s sorted (grouped) order. `collected` is
/// sorted by (group, kind, name, signature) so `## Exports` places a
/// definition next to its own members, but a module's summary is a different
/// job with a different rule: the smallest *signature* among a kind, not
/// "whichever entry the grouping happened to put first". Grouping sorts on the
/// bare name, and an uppercase type name (`SourceFile`) outranks a lowercase
/// function name (`walk`) there, so reading grouped order let a module's own
/// type steal the summary from the function the module is actually about
/// (`walk.rs`: `pub struct SourceFile` was winning over `pub fn walk(...)`).
/// Picking the min signature within each kind decouples selection from display
/// order. Do not "simplify" this back to `.find()` — that silently
/// reintroduces the bug.
///
/// The `or_else` chain also prefers `FreeDef`, then `Header`, then
/// `FreeValue` over a qualified method (`ItemKind::Member`), because a bare
/// method signature (`fn new(...)`) makes a poor, decontextualized summary.
/// This is a preference order, not an exclusion: the final rung falls back to
/// the minimum signature over every collected item with no kind filter at
/// all, so a qualified method can still win when the module has no
/// `FreeDef`, `Header`, or `FreeValue`.
fn pick_summary_fallback(collected: &[Collected]) -> Option<String> {
    let pick_min_signature = |want: ItemKind| {
        collected
            .iter()
            .filter(|(_, kind, ..)| *kind == want)
            .min_by(|a, b| a.3.cmp(&b.3))
    };
    pick_min_signature(ItemKind::FreeDef)
        .or_else(|| pick_min_signature(ItemKind::Header))
        .or_else(|| pick_min_signature(ItemKind::FreeValue))
        .or_else(|| pick_min_signature(ItemKind::Module))
        .or_else(|| collected.iter().min_by(|a, b| a.3.cmp(&b.3)))
        .map(|(_, _, _, sig)| sig.clone())
}

/// `pre_parsed` must be a tree of exactly `text`. The only caller that passes
/// one does so when `strip_rust_test_modules` reported that it spliced
/// nothing, so the bytes are provably the same.
fn extract_code(ext: &str, text: &str, pre_parsed: Option<Tree>) -> Option<CodeInfo> {
    let spec = lang_for_ext(ext)?;
    // Compiled once per process. A malformed query can no longer strip a
    // language of its symbols while the compile exits 0 — `QUERIES` panics on
    // a bad query, and `validate_queries` forces that before any output.
    let query = QUERIES.get(ext)?;
    let tree = if let Some(tree) = pre_parsed {
        tree
    } else {
        let mut parser = Parser::new();
        parser.set_language(&spec.language).ok()?;
        parser.parse(text, None)?
    };
    let idx = CaptureIdx {
        def: query.capture_index_for_name("def"),
        name: query.capture_index_for_name("name"),
        vis: query.capture_index_for_name("vis"),
        imp: query.capture_index_for_name("import"),
        item: query.capture_index_for_name("item"),
    };
    let exports = (spec.export_set)(&tree, text);

    // (group, kind, name, signature). The sort key groups a definition with
    // its own members: `class Article` and `Article.title: str` share the
    // group `Article`, so a class no longer scatters across the section
    // because `@` < `A` < `d`. Only the ORDER changes — `Entity::symbols`
    // stays a plain `Vec<String>`.
    let mut collected: Vec<Collected> = Vec::new();
    // Method names for `Entity::methods`. Gathered here, not as a second
    // projection over `collected`: that tuple carries no node kind, and a
    // `fn`, a field and an associated `const` of one impl are all `Member`,
    // told apart only by signature text, which is never parsed.
    // `is_function`'s node kinds are the seam's one concession: they name
    // "a function" in the grammars this file already dispatches on.
    let mut methods: Vec<String> = Vec::new();
    let mut imports = Vec::new();
    // Keyed by the `@name` node's start byte: a `pub fn` matches both a @def
    // and an @item pattern, and a JS @def (the `export_statement`) and its
    // @item (the declaration) share one name node.
    let mut items: BTreeMap<usize, CodeItem> = BTreeMap::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        let parts = split_captures(m, &idx, text, &mut imports);
        if let Some(def) = parts.def {
            let Placement::Scoped(chain) = (spec.placement)(def, text) else {
                continue;
            };
            let name_text = parts.name.and_then(|n| text.get(n.byte_range()));
            let keep = should_keep(parts.vis, &chain, name_text, exports.as_ref(), &spec);
            let shape = (spec.shape)(def);
            if keep {
                let owner = (!chain.is_empty()).then(|| chain.join(spec.owner_sep));
                // Resolved once, after `placement` has accepted the item, and
                // then used for both the signature and the classification —
                // the two used to re-derive the node's kind independently.
                let sig = build_signature(text, def, parts.name, owner.as_deref(), &spec, &shape);
                if !sig.is_empty() {
                    let kind = classify(parts.name, owner.as_deref(), shape.rank);
                    let group = group_key(owner.as_deref(), name_text, def, text, &spec);
                    let trait_impl = owner
                        .as_deref()
                        .is_some_and(super::extract_rust::is_trait_impl);
                    if kind == ItemKind::Member && is_function(def) && !trait_impl {
                        if let Some(n) = name_text {
                            methods.push(n.to_string());
                        }
                    }
                    collected.push((group, kind, name_text.unwrap_or("").to_string(), sig));
                }
            }
            merge_item(
                &mut items,
                code_item(def, parts.name, &chain, keep, text, &spec, &shape),
            );
        } else if let Some(def) = parts.item {
            let Placement::Scoped(chain) = (spec.placement)(def, text) else {
                continue;
            };
            let name_text = parts.name.and_then(|n| text.get(n.byte_range()));
            let public = !spec.query_gates_visibility
                && should_keep(None, &chain, name_text, exports.as_ref(), &spec);
            let shape = (spec.shape)(def);
            merge_item(
                &mut items,
                code_item(def, parts.name, &chain, public, text, &spec, &shape),
            );
        }
    }
    let mut items: Vec<CodeItem> = items.into_values().collect();
    items.sort_by(|a, b| {
        (a.start_line, a.end_line, &a.name).cmp(&(b.start_line, b.end_line, &b.name))
    });
    attach_call_sites(&mut items, tree.root_node(), text);
    // Sort by (group, kind, name, signature): a definition's group places it
    // next to its own members instead of scattering across the section under
    // plain lexicographic order on the rendered signature, and within a group
    // `ItemKind`'s derived `Ord` (FreeDef, Header < FreeValue, Member) puts a
    // class or an impl header ahead of its own members.
    collected.sort();
    // Still keyed on the signature alone. The sort key is a function of the
    // same captures that build the signature — equal signatures have equal
    // group, kind, and name — so equal signatures remain adjacent.
    collected.dedup_by(|a, b| a.3 == b.3);
    imports.sort();
    imports.dedup();

    let summary_fallback = pick_summary_fallback(&collected);

    // Second projection over the same items: the bare names of top-level
    // definitions, for `Wiki::search`'s defined-names field. `Module`,
    // `Header` and `Member` are excluded by kind — `pub mod x`, impl headers,
    // methods and fields never enter — so no text parsing of signatures is
    // needed downstream. A set: two `fn f()` in different inline modules
    // collapse to one `f`.
    let mut defined: Vec<String> = collected
        .iter()
        .filter(|(_, kind, ..)| matches!(kind, ItemKind::FreeDef | ItemKind::FreeValue))
        .map(|(_, _, name, _)| name.clone())
        .collect();
    defined.sort();
    defined.dedup();
    methods.sort();
    methods.dedup();

    let symbols: Vec<String> = collected.into_iter().map(|(_, _, _, sig)| sig).collect();

    let docstring = if ext == "py" {
        super::extract_python::python_docstring(&tree, text)
    } else {
        None
    };

    Some(CodeInfo {
        lang: spec.lang_name.to_string(),
        symbols,
        imports,
        defined,
        methods,
        items,
        docstring,
        summary_fallback,
    })
}

/// Gives each call in the tree to the narrowest function item whose lines
/// hold it. A call outside every function (a static initializer) is dropped.
fn attach_call_sites(items: &mut [CodeItem], root: Node, text: &str) {
    let fns: Vec<usize> = (0..items.len())
        .filter(|&i| matches!(items[i].kind, CodeItemKind::Fn | CodeItemKind::Method))
        .collect();
    if fns.is_empty() {
        return;
    }
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
        // `call` is Python's; every other grammar says `call_expression`.
        if !matches!(node.kind(), "call_expression" | "call") {
            continue;
        }
        let Some(site) = node
            .child_by_field_name("function")
            .and_then(|f| call_site(f, text))
        else {
            continue;
        };
        let line = node.start_position().row + 1;
        let holder = fns
            .iter()
            .filter(|&&i| items[i].start_line <= line && line <= items[i].end_line)
            .min_by_key(|&&i| items[i].end_line - items[i].start_line);
        if let Some(&i) = holder {
            items[i].call_sites.push(site);
        }
    }
}

/// What a call's `function` node names, in any grammar this file dispatches
/// on; `None` for a call through an expression (`f()()`, `(*p)()`), an
/// operator or a destructor.
fn call_site(f: Node, text: &str) -> Option<CallSite> {
    let field = |name: &str| f.child_by_field_name(name);
    let text_of = |n: Node| text.get(n.byte_range());
    match f.kind() {
        "identifier" => Some(CallSite {
            name: text_of(f)?.to_string(),
            via: CallVia::Bare,
        }),
        "generic_function" => call_site(field("function")?, text),
        "template_function" => call_site(field("name")?, text),
        // Rust `a::b::f`, C++ `a::b::f` (whose `name` nests the rest).
        "scoped_identifier" | "qualified_identifier" => {
            let inner = call_site(field("name")?, text)?;
            if inner.via != CallVia::Bare {
                return Some(inner);
            }
            let Some(scope) = field("path").or_else(|| field("scope")) else {
                return Some(inner);
            };
            // `Vec::<u8>` and `Foo<T>` are `Vec` and `Foo`.
            let scope = text_of(scope)?.split('<').next().unwrap_or("");
            let last = scope
                .trim_end_matches("::")
                .rsplit("::")
                .next()
                .unwrap_or("");
            let last = last.trim();
            Some(CallSite {
                via: CallVia::Path(last.to_string()),
                ..inner
            })
        }
        "field_expression" | "member_expression" | "attribute" | "selector_expression" => {
            let mut name = field("field")
                .or_else(|| field("property"))
                .or_else(|| field("attribute"))?;
            if name.kind() == "template_method" {
                name = name.child_by_field_name("name")?;
            }
            if !matches!(
                name.kind(),
                "field_identifier" | "property_identifier" | "identifier"
            ) {
                return None;
            }
            let recv = field("value")
                .or_else(|| field("argument"))
                .or_else(|| field("object"))
                .or_else(|| field("operand"))?;
            let recv_text = text_of(recv)?;
            let via = match recv.kind() {
                "self" | "this" => CallVia::SelfRecv,
                "identifier" if recv_text == "self" => CallVia::SelfRecv,
                "identifier" => CallVia::Member(Some(recv_text.to_string())),
                _ => CallVia::Member(None),
            };
            Some(CallSite {
                name: text_of(name)?.to_string(),
                via,
            })
        }
        _ => None,
    }
}

/// Whether a definition node is a function, in any grammar this file
/// dispatches on. A C++ header states its members as declarations whose
/// declarator is a function declarator — `void apply(const Order&);` is a
/// `field_declaration`, not a `function_definition`; the declarator test goes
/// through `extract_cpp::is_function_declarator`, which also accepts the
/// pointer/reference wrapper a member returning `Level*` or `Level&` puts in
/// the way. A JS/TS `const f = () => …` is a function through its value.
///
/// A `declaration` sharing several declarators (`int x, f();`) is judged by
/// its first, so it is a value there though a later declarator is a function.
///
/// Shared by `methods` and item kinds. The JS/TS/Go kinds never arrive as a
/// kept member `@def`, so they leave `methods` as it was.
pub(crate) fn is_function(def: Node) -> bool {
    match def.kind() {
        "function_item"
        | "function_signature_item"
        | "function_definition"
        | "function_declaration"
        | "generator_function_declaration"
        | "method_definition"
        | "method_declaration" => true,
        "field_declaration" | "declaration" => def
            .child_by_field_name("declarator")
            .is_some_and(super::extract_cpp::is_function_declarator),
        "variable_declarator" => def
            .child_by_field_name("value")
            .is_some_and(|v| matches!(v.kind(), "arrow_function" | "function_expression")),
        _ => false,
    }
}

/// An item's doc is one line in `items.json`, so it is cut to this many
/// chars, the trailing `…` included.
const ITEM_DOC_MAX_CHARS: usize = 120;

/// The first sentence of an item's raw doc text, capped at
/// `ITEM_DOC_MAX_CHARS`. Counts `chars()`, never bytes, so a multi-byte doc
/// is never split mid-scalar.
fn item_doc_line(raw: &str) -> Option<String> {
    let line = summarize(None, Some(raw), "", None)?;
    if line.chars().count() <= ITEM_DOC_MAX_CHARS {
        return Some(line);
    }
    let mut cut: String = line.chars().take(ITEM_DOC_MAX_CHARS - 1).collect();
    cut.push('…');
    Some(cut)
}

/// The most chars of an item's whole doc kept in `items.json`.
const ITEM_DOC_TEXT_MAX_CHARS: usize = 1000;

/// An item's whole raw doc on one line, capped at `ITEM_DOC_TEXT_MAX_CHARS`
/// (the `…` included); `None` when it is just `doc`.
fn item_doc_text(raw: &str, doc: Option<&str>) -> Option<String> {
    let mut text = one_line(raw);
    if text.chars().count() > ITEM_DOC_TEXT_MAX_CHARS {
        text = text.chars().take(ITEM_DOC_TEXT_MAX_CHARS - 1).collect();
        text.push('…');
    }
    (!text.is_empty() && doc != Some(text.as_str())).then_some(text)
}

/// The 0-based row a node's text really ends on. A node that ends with its
/// line's newline (a Rust `line_comment`) reports the start of the next row.
fn last_row(node: Node) -> usize {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row - 1
    } else {
        end.row
    }
}

/// Whether nothing but whitespace precedes `node` on its first line. A
/// comment trailing code (`int n; ///< Count.`, `var n = 5 // the limit`)
/// belongs to that code, so the lead walk must not read it as the next
/// item's doc, whatever its marker says.
fn opens_its_line(node: Node, text: &str) -> bool {
    let before = text.get(..node.start_byte()).unwrap_or_default();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    before[line_start..].trim().is_empty()
}

/// One definition as an item, keyed by its `@name` node's start byte, or
/// `None` when it is not an item: no `@name` (an impl header), rejected by
/// kind (a field, a variant, an associated const), or an empty signature.
///
/// The range climbs from the signature's start node over the contiguous
/// leads above it; a blank line, a lead trailing other code on its line, or
/// a `Stop` lead (other code) ends the climb, so a stray doc comment never
/// stretches the range. A plain block directly above the item (or its
/// attributes) extends the range only when `plain_doc` makes it the doc.
fn code_item(
    def: Node,
    name: Option<Node>,
    chain: &[String],
    public: bool,
    text: &str,
    spec: &LangSpec,
    shape: &Shape,
) -> Option<(usize, CodeItem)> {
    let name = name?;
    // JS exports match on the `export_statement`, which is no function: the
    // item is its declaration. `sig_start` climbs back to the statement, so
    // the range and signature are the declaration's own match's.
    let def = def
        .child_by_field_name("declaration")
        .filter(|_| def.kind() == "export_statement")
        .unwrap_or(def);
    let owner = (!chain.is_empty()).then(|| chain.join(spec.owner_sep));
    let kind = match (
        classify(Some(name), owner.as_deref(), shape.rank),
        is_function(def),
    ) {
        (ItemKind::Member, true) => CodeItemKind::Method,
        (ItemKind::FreeDef, true) => CodeItemKind::Fn,
        (ItemKind::FreeDef, false) => CodeItemKind::Type,
        // A Rust `type` alias ranks as a value in `## Exports`, yet is a
        // type, as a TS `type` and a C++ `using` are.
        (ItemKind::FreeValue, _) if def.kind() == "type_item" => CodeItemKind::Type,
        (ItemKind::FreeValue, _) => CodeItemKind::Value,
        (ItemKind::Module, _) => CodeItemKind::Module,
        (ItemKind::Header | ItemKind::Member, _) => return None,
    };
    // No owner splice: the signature reads as the source does.
    let signature = shared_declaration_signature(text, def, name, spec)
        .unwrap_or_else(|| build_signature(text, def, Some(name), None, spec, shape));
    if signature.is_empty() {
        return None;
    }

    let name_text = text.get(name.byte_range())?;
    let top = (spec.sig_start)(def);
    // `first` is where the range starts unless the plain block becomes the
    // doc; `cursor` is how far the walk has climbed, over that block too.
    let mut first = top;
    let mut cursor = top;
    let mut docs = Vec::new();
    let mut plain = Vec::new();
    // A `Doc` lead directly above the plain block ends the walk and keeps
    // the block from being the doc: the doc lead is the item's.
    let mut doc_above_plain = false;
    while let Some(lead) = cursor.prev_named_sibling() {
        if last_row(lead) + 1 < cursor.start_position().row {
            break;
        }
        // A lead that trails other nodes on its line (`#[a] #[b]`) is taken
        // with them only when they are all leads and the first opens the line.
        let mut group = vec![lead];
        while !opens_its_line(group[group.len() - 1], text) {
            let head = group[group.len() - 1];
            match head.prev_named_sibling() {
                Some(p) if last_row(p) == head.start_position().row => group.push(p),
                _ => break,
            }
        }
        let leads: Vec<Lead> = group.iter().map(|n| (spec.item_lead)(*n, text)).collect();
        if !opens_its_line(group[group.len() - 1], text)
            || leads.iter().any(|l| matches!(l, Lead::Stop))
        {
            break;
        }
        // A plain block is only ever the nearest thing above the item: a
        // plain lead above a doc, or anything but plain above the block,
        // ends the walk.
        let is_plain = |l: &Lead| matches!(l, Lead::Plain(_));
        if (leads.iter().any(is_plain) || !plain.is_empty())
            && (!docs.is_empty() || !leads.iter().all(is_plain))
        {
            doc_above_plain = !plain.is_empty() && leads.iter().any(|l| matches!(l, Lead::Doc(_)));
            break;
        }
        for lead in leads {
            match lead {
                Lead::Doc(d) => docs.push(d),
                Lead::Plain(p) => plain.push(p),
                Lead::Attr | Lead::Stop => {}
            }
        }
        cursor = group[group.len() - 1];
        if plain.is_empty() {
            first = cursor;
        }
    }
    docs.reverse();
    plain.reverse();
    // Doc comments, then the in-body doc, then the plain block; the range
    // takes in the plain block only when it is the doc used.
    let raw_doc = if docs.is_empty() {
        (spec.body_doc)(def, text).or_else(|| {
            if doc_above_plain {
                return None;
            }
            let lines: Vec<String> = plain
                .iter()
                .flat_map(|p| p.split('\n'))
                .map(str::to_string)
                .collect();
            let doc = plain_doc(&lines, name_text, cursor.start_position().row == 0)?;
            first = cursor;
            Some(doc)
        })
    } else {
        Some(docs.join("\n"))
    };

    let mut item = CodeItem {
        kind,
        name: name_text.to_string(),
        owner,
        signature,
        start_line: first.start_position().row + 1,
        end_line: last_row(top) + 1,
        doc: raw_doc.as_deref().and_then(item_doc_line),
        doc_text: None,
        public,
        twin: None,
        test: false,
        calls: Vec::new(),
        call_sites: Vec::new(),
    };
    item.doc_text = raw_doc
        .as_deref()
        .and_then(|r| item_doc_text(r, item.doc.as_deref()));
    Some((name.start_byte(), item))
}

/// Keep the first item seen for a name node. A later match of the same node
/// only ORs in its `public` flag: of a `pub fn`'s two matches, the @item one
/// alone says private.
fn merge_item(items: &mut BTreeMap<usize, CodeItem>, found: Option<(usize, CodeItem)>) {
    if let Some((key, item)) = found {
        let public = item.public;
        items
            .entry(key)
            .and_modify(|kept| kept.public |= public)
            .or_insert(item);
    }
}

/// The shared text pipeline for one fragment of a signature: Python's explicit
/// line continuations joined, internal whitespace collapsed, punctuation
/// artifacts tidied.
///
/// Shared by the head and by a retained value. A `line_continuation` can sit
/// *inside* a value node (`X = 1 + \`⏎`2` parses the continuation as a child of
/// the `binary_operator` that is the `right` field), and a wrapped argument
/// list inside a value collects the same `( ` / `, )` artifacts a wrapped
/// parameter list does. Running one pipeline over both is what keeps those two
/// cases correct; a value appended without it renders `1 + \ 2` and
/// `compute( 1, 2, )`.
fn normalize(raw: &str, spec: &LangSpec) -> String {
    // A backslash immediately followed by a newline is Python's explicit line
    // continuation: it is not whitespace, so it survives collapsing and
    // strands the trailing-strip loop one character short of the `=` it must
    // remove (`Z: int = \` never reduces to `Z: int`). Gated per-language:
    // JS and TS allow the identical sequence inside a string literal, and a
    // default-parameter value sits inside the retained signature span, so
    // joining it there would silently rewrite `"x\<newline>y"` to `"x y"`.
    //
    // CRLF line endings make the continuation sequence `\` `\r` `\n`, not
    // `\` `\n` — replace the CRLF form first, or a CRLF file leaves the
    // backslash unjoined, exactly the defect this join exists to prevent.
    let joined = if spec.join_continuations {
        raw.replace("\\\r\n", " ").replace("\\\n", " ")
    } else {
        raw.to_string()
    };
    tidy_punctuation(collapse_runs(&joined))
}

/// A comment node, in any of the five grammars: Rust's `line_comment` and
/// `block_comment`, everyone else's `comment`.
fn is_comment_kind(kind: &str) -> bool {
    kind.contains("comment")
}

/// Byte ranges of `root`'s descendants that satisfy `want`, clipped to
/// `[start, end)`, in source order and non-overlapping.
///
/// Only the outermost match is reported: descending into a matched node would
/// report a nested string inside a comment twice, and the caller treats a whole
/// matched range as one unit anyway.
fn descendant_spans(
    root: Node,
    start: usize,
    end: usize,
    want: fn(&str) -> bool,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut cursor = root.walk();
    let mut descend = true;
    loop {
        let node = cursor.node();
        let (a, b) = (node.start_byte(), node.end_byte());
        let matched = want(node.kind()) && b > start && a < end;
        if matched {
            out.push((a.max(start), b.min(end)));
        }
        // Skip the subtree of a match, and any subtree that cannot overlap.
        if descend && !matched && a < end && b > start && cursor.goto_first_child() {
            continue;
        }
        descend = true;
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                out.sort_unstable();
                return out;
            }
        }
    }
}

/// Render one fragment of a signature from source, with the parse tree in hand.
///
/// Replaces a flat `normalize` over the raw span. Three segment kinds, decided
/// structurally rather than by searching the text:
///
///   * a **comment** is dropped. A `///` on a field inside an enum variant's
///     body used to collapse into the signature — real crates render
///     `MatchErrorKind::UnsupportedStream { /// The match semantics … got: MatchKind, }`
///     — and a `//` line comment inside a one-line signature would comment out
///     everything after it for anyone who copied it. Also closes the two
///     documented Python risks: a comment between a decorator and its
///     definition, and one inside a retained value.
///   * a **string literal** is collapsed but never tidied, so a `" , "` stays
///     `" , "` instead of being rewritten to `", "`.
///   * everything else is collapsed and tidied, exactly as before.
///
/// Order is load-bearing. Segmentation reads node offsets, so it must happen
/// on the *raw* span: `join_continuations` is a `replace` that changes length
/// and would invalidate every offset if it ran first. It therefore runs per
/// segment, where it behaves identically.
///
/// Deliberately does not trim. `build_signature` splices an owner in at the
/// anchor node's start byte (`owner_anchor`), so it renders two fragments and concatenates them;
/// trimming each would delete the space between `pub fn` and the owner and
/// yield `pub fnWiki::search`. The caller trims once, at the end.
fn render_span(text: &str, root: Node, start: usize, end: usize, spec: &LangSpec) -> String {
    if start >= end {
        return String::new();
    }
    let comments = descendant_spans(root, start, end, is_comment_kind);
    let literals = descendant_spans(root, start, end, |k| {
        k.contains("string") || k.ends_with("char_literal") || k.ends_with("rune_literal")
    });

    let mut out = String::with_capacity(end - start);
    let mut pos = start;
    // Comments and literals never overlap, so one merged walk over both is
    // enough: at each step take whichever boundary comes first.
    let push_plain = |out: &mut String, a: usize, b: usize| {
        if a < b {
            if let Some(s) = text.get(a..b) {
                out.push_str(&normalize(s, spec));
            }
        }
    };
    while pos < end {
        let next_comment = comments.iter().find(|(a, b)| *b > pos && *a < end);
        let next_literal = literals.iter().find(|(a, b)| *b > pos && *a < end);
        let next = match (next_comment, next_literal) {
            (Some(c), Some(l)) => Some(if c.0 <= l.0 { (*c, true) } else { (*l, false) }),
            (Some(c), None) => Some((*c, true)),
            (None, Some(l)) => Some((*l, false)),
            (None, None) => None,
        };
        if let Some(((a, b), is_comment)) = next {
            push_plain(&mut out, pos, a.max(pos));
            if !is_comment {
                // Collapsed so a multi-line literal cannot break the
                // one-line invariant, but never tidied.
                if let Some(s) = text.get(a.max(pos)..b) {
                    out.push_str(&collapse_runs(s));
                }
            }
            pos = b.max(pos + 1);
        } else {
            push_plain(&mut out, pos, end);
            break;
        }
    }
    // Dropping a comment leaves the whitespace on either side of it in two
    // different segments, each collapsed to its own space — `pub /* c */ fn`
    // would render `pub  fn`. One final pass merges runs that a segment
    // boundary split. Idempotent for literals: they were already collapsed,
    // and this adds no tidying.
    collapse_runs(&out)
}

/// The signature of one name in a declaration with several declarators
/// (`int a, *b;`): the specifiers before the first declarator plus the
/// name's own declarator. `None` for any node with at most one declarator.
fn shared_declaration_signature(
    text: &str,
    def: Node,
    name: Node,
    spec: &LangSpec,
) -> Option<String> {
    let mut cursor = def.walk();
    let declarators: Vec<Node> = def
        .children_by_field_name("declarator", &mut cursor)
        .collect();
    let own = declarators
        .iter()
        .find(|d| d.byte_range().contains(&name.start_byte()))
        .filter(|_| declarators.len() > 1)?;
    let root = (spec.sig_start)(def);
    let head = render_span(
        text,
        root,
        root.start_byte(),
        declarators[0].start_byte(),
        spec,
    );
    let tail = render_span(text, root, own.start_byte(), own.end_byte(), spec);
    Some(format!("{head}{tail}").trim().to_string())
}

/// Build a one-line signature from a definition node: its source text up to
/// (not including) its `cut` node, with internal whitespace collapsed,
/// punctuation tidied, and language-specific trailing characters (Rust's `;`
/// and `=`, Python's `:`) removed.
///
/// When an `owner` is known, it is spliced in at the anchor node's start byte
/// (`owner_anchor`) — a byte-range operation, so there is no substring search
/// to mismatch.
fn build_signature(
    text: &str,
    def: Node,
    name: Option<Node>,
    owner: Option<&str>,
    spec: &LangSpec,
    shape: &Shape,
) -> String {
    let root = (spec.sig_start)(def);
    let start = root.start_byte();
    let end = shape
        .cut
        .map_or_else(|| def.end_byte(), |b| b.start_byte())
        .max(start)
        .min(text.len());
    // Each fragment is rendered from source with the tree in hand, then the
    // assembled whole is trimmed once — see `render_span` on why it must not
    // trim its own output.
    //
    // The head stops at the ANCHOR, not at the name: the two coincide for
    // every language but C++, where the anchor is the start of a qualifier the
    // source already wrote, and the bytes between the two are dropped so the
    // resolved owner replaces that qualifier rather than doubling it.
    let raw = match (owner, name) {
        (Some(owner), Some(name)) => {
            let anchor = (spec.owner_anchor)(name).start_byte();
            if anchor >= start && anchor <= name.start_byte() && name.start_byte() <= end {
                format!(
                    "{}{}{}{}",
                    render_span(text, root, start, anchor, spec),
                    owner,
                    spec.owner_sep,
                    render_span(text, root, name.start_byte(), end, spec)
                )
            } else {
                render_span(text, root, start, end, spec)
            }
        }
        _ => render_span(text, root, start, end, spec),
    };
    let mut sig = raw.trim().to_string();
    loop {
        match sig.chars().last() {
            Some(c) if spec.strip_trailing.contains(&c) => {
                sig.pop();
                sig = sig.trim_end().to_string();
            }
            _ => break,
        }
    }
    if let Some(value) = shape.value {
        let rendered = render_span(text, value, value.start_byte(), value.end_byte(), spec);
        sig = append_value(sig, rendered.trim(), shape.has_type);
    }
    sig
}

/// Characters of a retained value kept in a signature. Measured on the Python
/// audit corpora: the smallest round budget under which every real constant
/// survives intact, truncating only a multi-line prompt and a multi-line
/// user-agent string.
///
/// It governs a *typed* binding only as an omit-or-keep threshold — those are
/// never truncated. See `append_value`.
const VALUE_BUDGET: usize = 48;

/// Append a retained value to a head signature, bounded.
///
/// Over budget, what happens depends on what the head would still say without
/// the value. A Rust `const`/`static` and a Python annotated assignment both
/// keep a type, so the value is dropped and the reader loses nothing they could
/// not already see — and a truncated aggregate or raw string is measurably
/// worse than the type it would replace: 48 characters of this crate's own
/// `query_src` is `r#" ; One pattern covers module constants AND cl…`, a source
/// comment from inside the string that reads as documentation of the constant.
///
/// An unannotated Python assignment keeps nothing, so its value is truncated
/// rather than dropped: a bare `SYSTEM_PROMPT` has no kind, no type and no
/// value. Counts `chars()`, never bytes — the audit corpus contains a Cyrillic
/// prompt literal that a byte slice would split mid-scalar and panic on.
fn append_value(head: String, value: &str, has_type: bool) -> String {
    if value.chars().count() <= VALUE_BUDGET {
        return format!("{head} = {value}");
    }
    if has_type {
        return head;
    }
    let kept: String = value.chars().take(VALUE_BUDGET).collect();
    format!("{head} = {kept}…")
}

/// Remove the artifacts whitespace collapsing leaves around the punctuation of
/// a multi-line parameter list: `fn f( a: u8, b: u8, )` becomes
/// `fn f(a: u8, b: u8)`. The trailing comma before a collapsed closing
/// delimiter always arrives as `, )` (with a space, from the newline that
/// separated them), so matching that sequence is enough to remove it; a
/// genuine one-element tuple written `(1,)` has no such space and is left
/// alone.
///
/// Known limitation, still open: a one-element tuple that was itself wrapped
/// across lines collapses to `( u8, )` and is reduced to `(u8)`, losing its
/// arity. Withholding string literals does not help here — the text is
/// identical to a wrapped one-parameter list, which must reduce to `(u8)`, so
/// telling them apart needs the tuple's own node kind, not just its span.
/// Measured at 4 occurrences across 154 real stdlib modules; deferred as not
/// worth the machinery.
///
/// Unlike `placement`, `vis_filter` and `owner_sep`, this pass is not a
/// `LangSpec` field: it runs inside `render_span` for all five languages, not
/// just Rust.
///
/// It is still a plain substring substitution, but it is no longer applied to
/// the whole signature: `render_span` withholds string-literal spans from it,
/// so a Python default `gamma=" )"` and a Rust `pub const SEP: &str = " , "`
/// now keep their own punctuation. Pinned by
/// `a_rust_string_value_keeps_its_own_punctuation` and by the Python fixture in
/// `wrapped_parameter_lists_are_tidied_in_every_language`.
fn tidy_punctuation(mut sig: String) -> String {
    for (from, to) in [("( ", "("), (", )", ")"), (" )", ")"), (" ,", ",")] {
        while sig.contains(from) {
            sig = sig.replace(from, to);
        }
    }
    sig
}

/// Collapse each run of whitespace to a single space, **without** trimming.
///
/// Trimming here would be wrong now that a signature is rendered as several
/// fragments and concatenated: it would eat the space between `pub fn` and a
/// spliced-in owner. `build_signature` trims the assembled signature once.
fn collapse_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

/// Module-level doc comment, one paragraph, if present. The fallback for
/// languages without an AST-verified docstring convention (everything but
/// Python).
///
/// Empty doc lines are SKIPPED rather than terminating the scan: a Doxygen
/// block opens with a bare `/**`, which strips to nothing, and aborting there
/// left the commonest C++ comment shape with no docstring at all.
///
/// Skipping forward past a NON-doc line (a license/SPDX banner) in search of
/// the real doc beneath it is a C++-only allowance, gated on `cpp` below —
/// and further bounded by `is_banner_shaped`, so it skips only lines that are
/// still comments, never actual code. Every other language keeps the
/// stricter rule: the doc comment must open on the file's first non-blank
/// line, or there is none. Without the language gate, this scan would read
/// the whole file forward hunting for the first `///`/`//!`/`/*`-prefixed
/// line anywhere in it — which is exactly what a real corpus does contain, on
/// some unrelated item deep in the file — and hand that back as the MODULE's
/// summary. Without the `is_banner_shaped` bound, C++ has the identical bug
/// narrowed rather than fixed: it would walk straight through
/// `class OrderBook {` and `public:` to reach a MEMBER's doc comment and
/// report that as the file's.
fn leading_doc(text: &str, ext: &str) -> Option<String> {
    let cpp = matches!(ext, "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "h");
    // Tracks whether the line about to be stripped sits inside a still-open
    // Doxygen block (`/**` or `/*!` not yet closed by `*/`) — the only
    // context in which a bare `*` continuation is real doc content. Without
    // it, `cpp_doc_line` cannot tell a Doxygen continuation line apart from
    // the `*`-prefixed continuation of a plain `/* ... */` license block,
    // and the banner's own text gets collected as if it were the doc body.
    // See `cpp_doc_line`'s own doc for the mechanics.
    let mut in_doxygen_block = false;
    let mut strip = |line: &str| -> Option<String> {
        let doc = match ext {
            "rs" => line
                .strip_prefix("//!")
                .or_else(|| line.strip_prefix("///")),
            "py" => line
                .strip_prefix("\"\"\"")
                .map(|s| s.trim_end_matches("\"\"\"")),
            "js" | "ts" | "jsx" | "tsx" | "vue" | "svelte" | "go" => {
                line.strip_prefix("//").or_else(|| line.strip_prefix("/*"))
            }
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "h" => {
                return super::extract_cpp::cpp_doc_step(line, &mut in_doxygen_block);
            }
            _ => None,
        };
        doc.map(|d| d.trim().to_string())
    };

    let mut lines = text.lines().map(str::trim).skip_while(|l| l.is_empty());
    let mut collected: Vec<String> = Vec::new();
    for line in lines.by_ref() {
        match strip(line) {
            Some(d) if !d.is_empty() => collected.push(d),
            // An empty doc line (a bare `/**`, or a lone `*/`) is a gap
            // within the paragraph, never its end — skip it either way.
            Some(_) => {}
            // Still hunting past a license/SPDX banner — C++ only, and only
            // while the line itself is still comment-shaped. `cpp_doc_line`
            // returns `None` for a `// Copyright` banner and for ordinary
            // code alike, so the banner-shaped check below is what tells
            // "skip past a license header" apart from "skip past
            // `#include`, a class's opening brace, or a whole member's
            // body" — none of which documents the FILE.
            None if collected.is_empty() && cpp && is_banner_shaped(line) => {}
            // Every other language, and C++ once real code appears: no doc
            // comment opens this file.
            None if collected.is_empty() => break,
            // A non-doc line ends an already-open paragraph.
            None => break,
        }
    }
    (!collected.is_empty()).then(|| collected.join("\n"))
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
fn is_banner_shaped(line: &str) -> bool {
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
pub(crate) fn cpp_doc_line(line: &str, in_doxygen_block: bool) -> Option<String> {
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
    if cleaned.chars().any(char::is_alphanumeric) {
        Some(cleaned)
    } else {
        Some(String::new())
    }
}

/// Filenames that say nothing about the module's own identity: the language
/// names such a module after its DIRECTORY.
///
/// `index.js` / `index.ts` is the same shape for JS and TS. It was left out
/// while those languages were unfinished, but leaving it out is not neutral:
/// the stem falls through to `index`, so the module page is named after a file
/// no importer mentions, and the bare directory name stays free for a
/// like-named test (`tests/backends.test.ts`) to claim — which then collects
/// the importer's edge. On a 49-page TypeScript corpus that was 15 edges into
/// tests and 22 real module imports resolving nowhere.
///
/// `index.tsx` / `index.jsx` is the same barrel file in a component tree.
/// `.mjs`/`.cjs` want the same entry but are not compiled at all yet, so they
/// would be dead rows here.
const DIRECTORY_MODULES: &[(&str, &str)] = &[
    ("rs", "mod"),
    ("py", "__init__"),
    ("ts", "index"),
    ("js", "index"),
    ("tsx", "index"),
    ("jsx", "index"),
];

/// The identifier a code file's module is known by, before any casing: the
/// base name up to its first `.`, or the parent directory's name for a
/// directory-module file (`mod.rs`, `__init__.py`) that has a parent. Shared
/// by page naming (`derive_code_name`) and by `graph.rs` (import resolution
/// and the code-shaped mention filter), so there is exactly one definition
/// of how a code page is named.
pub(crate) fn module_stem(rel_path: &str) -> &str {
    let base = basename(rel_path);
    let stem = base.split('.').next().unwrap_or(base);
    let ext = rel_path.rsplit('.').next().unwrap_or("");
    if DIRECTORY_MODULES
        .iter()
        .any(|(e, m)| *e == ext && *m == stem)
    {
        // At the corpus root there is no directory to borrow from, so the
        // stem stands.
        if let Some(parent) = rel_path.rsplit('/').nth(1) {
            return parent;
        }
    }
    stem
}

/// A code page's name: the title-cased `module_stem`.
///
/// `tests/common/mod.rs` named "Mod" was unreachable. Every importer refers to
/// it as `common` — `mod common;`, `use common::helper` — so the page's own
/// name appeared in no other page's body, and neither the phrase index nor
/// import resolution could link it. With several such files present they also
/// collided on the id `mod` and were qualified to `common_mod` / `formats_mod`,
/// which matched nothing either. Naming it "Common" lets a `mod common;`
/// declaration or a `common::` path link it, and `graph::ImportResolver`
/// reaches it through the same stem.
fn derive_code_name(rel_path: &str) -> String {
    title_from_slug(module_stem(rel_path))
}

/// Assertions shared by every language's extraction tests.
///
/// The two are deliberately asymmetric, and both pick the *strict* option for
/// their direction: `assert_has` matches a signature exactly, so it cannot be
/// satisfied by a longer signature that merely contains the wanted text, and
/// `assert_lacks` matches a substring, so it fails on any signature that
/// mentions the name at all. Loosening either — an `assert_has` that accepted
/// a substring especially — would let tests keep passing while the extractor
/// regressed.
///
/// They take an already-extracted symbol list rather than source text, so a
/// test with several assertions extracts once.
#[cfg(test)]
pub(crate) mod testutil {
    use crate::model::CodeItem;

    /// The exact signature `want` must be among `symbols`.
    pub(crate) fn assert_has(symbols: &[String], want: &str) {
        assert!(
            symbols.iter().any(|s| s == want),
            "want {want:?} in {symbols:?}"
        );
    }

    /// No signature may so much as mention `unwanted`.
    pub(crate) fn assert_lacks(symbols: &[String], unwanted: &str) {
        assert!(
            !symbols.iter().any(|s| s.contains(unwanted)),
            "{unwanted:?} must not appear in {symbols:?}"
        );
    }

    /// One `"{start}-{end} {kind} {owner::}name {pub|priv}"` line per item,
    /// so a test pins every item's range, kind and flag in one comparison.
    pub(crate) fn item_lines(items: &[CodeItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| {
                let name = match &i.owner {
                    Some(owner) => format!("{owner}::{}", i.name),
                    None => i.name.clone(),
                };
                let vis = if i.public { "pub" } else { "priv" };
                format!(
                    "{}-{} {} {name} {vis}",
                    i.start_line,
                    i.end_line,
                    i.kind.as_str()
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_of_lowercases_and_source_rank_totally_orders_headers() {
        assert_eq!(ext_of("src/Book.H"), "h");
        assert_eq!(ext_of("src/MAIN.CPP"), "cpp");
        assert_eq!(ext_of("Makefile"), "");
        assert_eq!(ext_of(".gitignore"), "");
        assert_eq!(ext_of("a/b.c/d"), "");

        assert_eq!(source_rank("src/orderbook.h"), 0);
        assert_eq!(source_rank("src/orderbook.hpp"), 1);
        assert_eq!(source_rank("src/orderbook.cpp"), 2);
        assert_eq!(source_rank("src/lib.rs"), 2);
        // Uppercase must rank as its lowercase form, or `.CPP` would outrank `.h`.
        assert_eq!(source_rank("src/Orderbook.H"), 0);
    }

    #[test]
    fn uppercase_extension_still_extracts() {
        let e = CodeExtractor.extract("src/Book.H", "class Book {};\n");
        match &e.kind {
            crate::model::SourceKind::Code { lang } => assert_eq!(lang, "cpp"),
            other => panic!("expected Code kind, got {other:?}"),
        }
    }

    #[test]
    fn defined_keeps_top_level_names_only() {
        // Module declaration, struct field, trait-impl method, trait with a
        // bound, typed const, free fn. Only the four top-level definitions
        // survive: `a` (Module), `f` (field = Member) and `m` (method =
        // Member) are excluded by `ItemKind`, never by text inspection.
        let src = "pub mod a;\npub struct S { pub f: u8 }\nimpl T for S {\n    fn m(&self) {}\n}\npub trait Tr: Send {}\npub const C: u32 = 1;\npub fn g() {}\n";
        let e = CodeExtractor.extract("s.rs", src);
        assert_eq!(
            e.defined,
            vec!["C", "S", "Tr", "g"],
            "defined: {:?}",
            e.defined
        );
    }

    #[test]
    fn defined_is_a_sorted_set_and_inline_module_items_are_unqualified() {
        let src = "pub mod inner {\n    pub fn in_mod() {}\n}\npub mod other {\n    pub fn in_mod() {}\n}\npub fn in_mod() {}\n";
        let e = CodeExtractor.extract("m.rs", src);
        assert_eq!(e.defined, vec!["in_mod"], "defined: {:?}", e.defined);
    }

    #[test]
    fn defined_ignores_trait_impl_headers_and_qualified_methods() {
        let src = "pub struct TextExtractor;\nimpl Extractor for TextExtractor {\n    fn extensions(&self) -> &[&str] { &[] }\n    fn extract(&self, p: &str) -> u8 { 0 }\n}\n";
        let e = CodeExtractor.extract("text.rs", src);
        assert_eq!(e.defined, vec!["TextExtractor"], "defined: {:?}", e.defined);
    }

    #[test]
    fn methods_keep_inherent_and_trait_declaration_functions_only() {
        // Inherent impl: kept. Trait impl: dropped (owner `<S as T>`). Trait
        // declaration: kept (`function_signature_item`, the trait page
        // defines the API). A field and an associated const are `Member`
        // but not functions. `defined` is unchanged by the new field.
        // `m` collides between the trait impl and the trait declaration, so
        // its presence alone wouldn't pin the trait-impl exclusion; a second
        // trait impl with a uniquely named method (`only_in_impl`) does that
        // instead, via its absence from `e.methods`.
        let src = "pub struct S { pub f: u8 }\nimpl S {\n    pub fn new() -> S { S { f: 0 } }\n    pub fn get(&self) -> u8 { self.f }\n}\nimpl T for S {\n    fn m(&self) {}\n}\nimpl U for S {\n    fn only_in_impl(&self) {}\n}\npub trait T {\n    const K: u32;\n    fn m(&self);\n}\n";
        let e = CodeExtractor.extract("s.rs", src);
        assert_eq!(
            e.methods,
            vec!["get", "m", "new"],
            "methods: {:?}",
            e.methods
        );
        assert!(
            !e.methods.contains(&"only_in_impl".to_string()),
            "trait-impl methods must be excluded: {:?}",
            e.methods
        );
        assert_eq!(e.defined, vec!["S", "T"], "defined: {:?}", e.defined);
    }

    #[test]
    fn methods_capture_python_class_methods_and_drop_private_ones() {
        // `should_keep` already applies `keep_python_public` to members, so
        // `_private` and `__repr__` never reach `collected`; pinned here so
        // a change there shows up as a `methods` change.
        let py = "class Registry:\n    def register(self, x):\n        pass\n    def _private(self):\n        pass\n    def __repr__(self):\n        return \"\"\n\ndef free():\n    pass\n";
        let e = CodeExtractor.extract("t.py", py);
        assert_eq!(e.methods, vec!["register"], "methods: {:?}", e.methods);
        assert_eq!(
            e.defined,
            vec!["Registry", "free"],
            "defined: {:?}",
            e.defined
        );
    }

    #[test]
    fn summary_fallback_follows_signature_order_not_grouped_display_order() {
        // `walk.rs`'s real shape: a `pub struct` whose name sorts ahead of a
        // `pub fn`'s under the *grouping* key — uppercase `S` (0x53) sorts
        // before lowercase `w` (0x77), so `SourceFile`'s group leads
        // `## Exports`. If the summary fallback read that same grouped
        // order, the struct would steal the module's summary from the
        // function it's actually about. The fallback must instead compare
        // signatures directly:
        // `pub fn walk() -> u8` sorts before `pub struct SourceFile`
        // lexicographically (`f` < `s`), so it must win the summary even
        // though it displays second in `## Exports`.
        let src = "pub struct SourceFile;\npub fn walk() -> u8 { 0 }\n";
        let e = CodeExtractor.extract("walk.rs", src);
        assert_eq!(
            e.symbols,
            vec![
                "pub struct SourceFile".to_string(),
                "pub fn walk() -> u8".to_string(),
            ],
            "grouped display order: {:?}",
            e.symbols
        );
        assert_eq!(
            e.summary.as_deref(),
            Some("pub fn walk() -> u8"),
            "the summary must follow signature order, not grouped display order: {:?}",
            e.symbols
        );
    }

    #[test]
    fn a_directory_module_is_named_for_its_directory() {
        // Rust's `mod.rs` and Python's `__init__.py` carry no identity in
        // their own filename — both languages refer to such a module by its
        // DIRECTORY. Naming the page "Mod"/"Init" made it unreachable: an
        // importer's body says `mod common; use common::helper`, which never
        // contains the page's own name, so neither the phrase index nor
        // import resolution could ever link it. Measured on this repo,
        // `tests/common/mod.rs` was a permanent orphan.
        let e = CodeExtractor.extract("tests/common/mod.rs", "pub fn helper() -> u8 { 0 }\n");
        assert_eq!(e.name, "Common");
        assert_eq!(e.id, "common");

        let e = CodeExtractor.extract("pkg/__init__.py", "def exported():\n    pass\n");
        assert_eq!(e.name, "Pkg");
        assert_eq!(e.id, "pkg");
    }

    #[test]
    fn a_directory_module_at_the_corpus_root_keeps_its_own_stem() {
        // Nothing to borrow a name from, so the old behaviour stands.
        let e = CodeExtractor.extract("mod.rs", "pub fn f() {}\n");
        assert_eq!(e.name, "Mod");
        let e = CodeExtractor.extract("__init__.py", "def f():\n    pass\n");
        assert_eq!(e.name, "Init");
    }

    #[test]
    fn an_ordinary_file_is_still_named_for_itself_not_its_directory() {
        // Guards the overcorrection: only the two directory-module markers
        // borrow the parent name.
        let e = CodeExtractor.extract("src/formats/code.rs", "pub fn f() {}\n");
        assert_eq!(e.name, "Code");
        let e = CodeExtractor.extract("src/models.py", "def f():\n    pass\n");
        assert_eq!(e.name, "Models");
    }

    #[test]
    fn a_mod_file_in_another_language_is_not_treated_as_a_directory_module() {
        // `mod` is only a directory-module marker for Rust; a `mod.py` is an
        // ordinary module named `mod`.
        let e = CodeExtractor.extract("pkg/mod.py", "def f():\n    pass\n");
        assert_eq!(e.name, "Mod");
    }

    #[test]
    fn multi_line_parameter_lists_are_tidied() {
        let src = "pub fn generate(\n    out: &Path,\n    n: usize,\n) -> Result<(), Error> { todo!() }\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub fn generate(out: &Path, n: usize) -> Result<(), Error>"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn one_element_tuple_survives_tidying() {
        let src = "pub type Single = (u8,);\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols.iter().any(|s| s == "pub type Single = (u8,)"),
            "a one-element tuple is not a parenthesized value: {:?}",
            e.symbols
        );
    }

    #[test]
    fn non_rust_bodies_are_untouched() {
        let src = "def test_visible():\n    pass\n";
        let e = CodeExtractor.extract("t.py", src);
        assert_eq!(e.body, src);
    }

    #[test]
    fn each_language_keeps_its_own_scope_rules() {
        // Python now qualifies members and drops function-local definitions.
        let py = "class Registry:\n    def register(self, x):\n        pass\n\ndef outer():\n    def inner():\n        pass\n    return inner\n";
        let e = CodeExtractor.extract("t.py", py);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "def Registry.register(self, x)"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "def outer()"),
            "{:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("inner")),
            "function-local def must be dropped: {:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("::")),
            "Python must use `.`, never Rust's `::`: {:?}",
            e.symbols
        );

        // The summary fallback is shared machinery that Task 9 reordered.
        // Pin it per language so a future reordering cannot move it silently.
        let e = CodeExtractor.extract("t.py", py);
        assert_eq!(
            e.summary.as_deref(),
            Some("class Registry"),
            "symbols: {:?}",
            e.symbols
        );

        // Go has no owner resolution and no scope guard: a nested func and a
        // method both stay exactly as they are today.
        let go = "package main\n\nfunc Foo(a int) string {\n\treturn \"\"\n}\n";
        let e = CodeExtractor.extract("m.go", go);
        assert!(
            e.symbols.iter().any(|s| s == "func Foo(a int) string"),
            "symbols: {:?}",
            e.symbols
        );
        assert_eq!(e.summary.as_deref(), Some("func Foo(a int) string"));

        let js = "export function foo(a, b) {\n  return a + b;\n}\n";
        let e = CodeExtractor.extract("m.js", js);
        assert_eq!(e.symbols, vec!["export function foo(a, b)".to_string()]);
        assert_eq!(e.summary.as_deref(), Some("export function foo(a, b)"));

        let ts = "export class Widget {\n  x = 1;\n}\n";
        let e = CodeExtractor.extract("m.ts", ts);
        assert_eq!(e.symbols, vec!["export class Widget".to_string()]);
        assert_eq!(e.summary.as_deref(), Some("export class Widget"));
    }

    #[test]
    fn module_stem_is_the_name_importers_use() {
        assert_eq!(module_stem("text.rs"), "text");
        assert_eq!(module_stem("src/formats/extract_rust.rs"), "extract_rust");
        assert_eq!(module_stem("foo.test.rs"), "foo");
        assert_eq!(module_stem("tests/common/mod.rs"), "common");
        assert_eq!(module_stem("pkg/__init__.py"), "pkg");
        // A directory module at the corpus root has no directory to borrow.
        assert_eq!(module_stem("mod.rs"), "mod");
        // The name every page derives from it is the title-cased stem.
        assert_eq!(derive_code_name("tests/common/mod.rs"), "Common");
        assert_eq!(derive_code_name("src/extract_rust.rs"), "Extract Rust");
    }

    #[test]
    fn index_is_a_directory_module_the_way_mod_and_dunder_init_are() {
        // `import './backends/index.js'` names the module `backends`, exactly
        // as `mod common;` names `tests/common/mod.rs`.
        assert_eq!(module_stem("src/backends/index.ts"), "backends");
        assert_eq!(module_stem("web/src/api/index.js"), "api");
        assert_eq!(derive_code_name("src/backends/index.ts"), "Backends");
        // A directory module at the corpus root has no directory to borrow.
        assert_eq!(module_stem("index.ts"), "index");
    }

    #[test]
    fn wrapped_parameter_lists_are_tidied_in_every_language() {
        // `tidy_punctuation` is not gated by `LangSpec` the way owner/vis
        // machinery is — it runs inside `build_signature` for all five
        // languages. Pin that as intended behavior, not something left to be
        // discovered. The fixture also guards the boundary: `beta=(1,)` is
        // real punctuation and must still be tidied to `(1,)`, while
        // `gamma=" )"` is inside a string literal and must survive untouched.
        // One fixture, both sides of the rule.
        let py = "def wrapped(\n    alpha,\n    beta=(1,),\n    gamma=\" )\",\n):\n    pass\n";
        let e = CodeExtractor.extract("t.py", py);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "def wrapped(alpha, beta=(1,), gamma=\" )\")"),
            "symbols: {:?}",
            e.symbols
        );

        let go =
            "package main\n\nfunc Wrapped(\n\ta int,\n\tb int,\n) string {\n\treturn \"\"\n}\n";
        let e = CodeExtractor.extract("m.go", go);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "func Wrapped(a int, b int) string"),
            "symbols: {:?}",
            e.symbols
        );

        // The name promises "every language" but the fixtures above cover
        // only Python and Go — JS, TS, and Rust close the gap.
        let js = "export function wrapped(\n  a,\n  b,\n) {\n  return a + b;\n}\n";
        let e = CodeExtractor.extract("t.js", js);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "export function wrapped(a, b)"),
            "symbols: {:?}",
            e.symbols
        );

        let ts = "export function wrapped(\n  a: number,\n  b: number,\n): number {\n  return a + b;\n}\n";
        let e = CodeExtractor.extract("t.ts", ts);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "export function wrapped(a: number, b: number): number"),
            "symbols: {:?}",
            e.symbols
        );

        let rust = "pub fn wrapped(\n    a: u8,\n    b: u8,\n) -> u8 {\n    a + b\n}\n";
        let e = CodeExtractor.extract("t.rs", rust);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub fn wrapped(a: u8, b: u8) -> u8"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn javascript_string_literal_line_continuations_survive_collapsing() {
        // Python's `\`-newline join (added so `Z: int = \` reduces to
        // `Z: int`) must NOT be applied to JS/TS: the identical sequence is a
        // legal string-literal continuation there, and a default-parameter
        // value sits inside the retained signature span (the cut stops at
        // `body`, after the parameter list). Joining it would silently turn
        // `"x\<newline>y"` into `"x y"`. `join_continuations` gates this
        // per-language; only the whitespace-collapse (newline -> space) that
        // every language already gets should touch this text.
        let js = "export function foo(a = \"x\\\ny\") {\n  return a;\n}\n";
        let e = CodeExtractor.extract("t.js", js);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "export function foo(a = \"x\\ y\")"),
            "the backslash must survive: {:?}",
            e.symbols
        );
    }

    #[test]
    fn every_registered_language_query_compiles() {
        // Forcing the statics IS the check: `QUERIES` panics on a malformed
        // query, and this is the same call `compile` makes before writing any
        // output. A language present in `CODE_EXTENSIONS` but missing a
        // `LangSpec` panics here too.
        validate_queries();
        for ext in CODE_EXTENSIONS {
            assert!(QUERIES.contains_key(ext), "no compiled query for {ext}");
        }
    }

    #[test]
    fn the_query_registry_covers_exactly_the_claimed_extensions() {
        // A language accepted by `extensions()` but absent from `QUERIES`
        // would extract nothing while the compile still exits 0 — the silent
        // failure the old per-file `Query::new` path allowed.
        let claimed: BTreeSet<&str> = CodeExtractor.extensions().iter().copied().collect();
        let compiled: BTreeSet<&str> = QUERIES.keys().copied().collect();
        assert_eq!(claimed, compiled);
    }

    #[test]
    fn a_doc_comment_must_open_on_the_first_line_for_every_non_cpp_language() {
        // `leading_doc`'s restructuring must not turn Rust, Python, JS, TS or
        // Go into a C++-style banner-skipping scan: a `///` doc comment
        // buried past the top of the file belongs to whatever item follows
        // it, not to the module, and must not become the module's summary.
        let rs = "use crate::graph::build;\n/// Not the module's doc.\npub fn f() {}\n";
        assert_eq!(leading_doc(rs, "rs"), None);

        let js = "const x = 1;\n/// Not the module's doc.\nfunction f() {}\n";
        assert_eq!(leading_doc(js, "js"), None);

        let go = "package main\n// Not the module's doc.\nfunc F() {}\n";
        assert_eq!(leading_doc(go, "go"), None);
    }

    #[test]
    fn a_blank_doc_line_joins_the_paragraph_on_either_side_of_it() {
        // A blank `//!` line is Rust's own paragraph break within one doc
        // comment run, not the end of the comment: aborting there (as the
        // unrestructured `leading_doc` did, via its first-line `?`) lost the
        // elaboration that followed it.
        let rs = "//! Summary line.\n//!\n//! Elaboration line.\nfn f() {}\n";
        assert_eq!(
            leading_doc(rs, "rs").as_deref(),
            Some("Summary line.\nElaboration line.")
        );
    }

    #[test]
    fn is_test_path_follows_the_path_rule() {
        use crate::model::is_test_path;
        for p in [
            "tests/a.rs",
            "src/tests/x.cpp",
            "test_fsm.cpp",
            "fsm_test.go",
            "a/fsm_tests.rs",
        ] {
            assert!(is_test_path(p), "{p}");
        }
        for p in ["src/attest.rs", "src/testing.rs", "contest/a.rs"] {
            assert!(!is_test_path(p), "{p}");
        }
    }

    fn doc_of(src: &str) -> (Option<String>, Option<String>) {
        let i = CodeExtractor.extract("src/a.rs", src).items.remove(0);
        (i.doc, i.doc_text)
    }

    #[test]
    fn doc_text_keeps_the_whole_doc() {
        let (doc, text) = doc_of(
            "/// Pure decision.\n///\n/// * `creds_present` — owner has a futures key.\npub fn decide() {}\n",
        );
        assert_eq!(doc.as_deref(), Some("Pure decision."));
        let text = text.unwrap();
        assert!(
            text.contains("creds_present") && text.contains("futures key"),
            "{text}"
        );
    }

    #[test]
    fn doc_text_is_none_for_a_one_sentence_doc() {
        assert_eq!(doc_of("/// Only this.\nfn f() {}\n").1, None);
    }

    #[test]
    fn doc_text_caps_on_a_char_boundary() {
        let src = format!("/// {}\nfn f() {{}}\n", "é".repeat(500).replace('é', "éé "));
        let text = doc_of(&src).1.unwrap();
        assert!(
            text.chars().count() <= 1000 && text.ends_with('…'),
            "{}",
            text.len()
        );
    }

    fn test_flags(path: &str, src: &str) -> Vec<(String, bool)> {
        let e = CodeExtractor.extract(path, src);
        e.items.into_iter().map(|i| (i.name, i.test)).collect()
    }

    #[test]
    fn rust_items_inside_cfg_test_are_tests() {
        let src = "fn live() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n    fn helper() {}\n}\n";
        let flags = test_flags("src/a.rs", src);
        for (name, want) in [("live", false), ("t", true), ("helper", true)] {
            assert!(
                flags.contains(&(name.to_string(), want)),
                "{name}: {flags:?}"
            );
        }
    }

    #[test]
    fn rust_test_attribute_marks_a_test_outside_cfg_test() {
        let flags = test_flags(
            "src/a.rs",
            "#[tokio::test]\nasync fn x() {}\n#[test]\nfn y() {}\nfn z() {}\n",
        );
        assert_eq!(
            flags,
            [("x".into(), true), ("y".into(), true), ("z".into(), false)]
        );
    }

    #[test]
    fn items_of_a_test_file_are_tests_in_every_language() {
        assert_eq!(
            test_flags("tests/a.go", "package a\nfunc F() {}\n"),
            [("F".into(), true)]
        );
        assert_eq!(
            test_flags("src/a.go", "package a\nfunc F() {}\n"),
            [("F".into(), false)]
        );
    }

    /// The call sites of item `name` in `src`, one per call, written
    /// `name`, `q::name`, `self.name`, `x.name` or `_.name`, sorted.
    fn sites(path: &str, src: &str, name: &str) -> Vec<String> {
        let e = CodeExtractor.extract(path, src);
        let item = e.items.iter().find(|i| i.name == name).expect("the item");
        let mut out: Vec<String> = item
            .call_sites
            .iter()
            .map(|c| match &c.via {
                CallVia::Bare => c.name.clone(),
                CallVia::Path(q) => format!("{q}::{}", c.name),
                CallVia::SelfRecv => format!("self.{}", c.name),
                CallVia::Member(x) => format!("{}.{}", x.as_deref().unwrap_or("_"), c.name),
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn call_sites_name_their_targets_in_each_grammar() {
        let rs = "struct S;\nimpl S {\n    fn f(&self) {\n        g();\n        a::b::h();\n        Self::k();\n        self.m();\n        x.n();\n        Vec::<u8>::new();\n        v.iter().map(|y| y);\n    }\n}\nfn g() {}\n";
        assert_eq!(
            sites("src/s.rs", rs, "f"),
            ["Self::k", "Vec::new", "_.map", "b::h", "g", "self.m", "v.iter", "x.n"]
        );
        // The narrowest function holds a call: `g`'s own body is empty.
        assert!(sites("src/s.rs", rs, "g").is_empty());
        let cpp = "void S::f() {\n  g();\n  ns::h();\n  this->m();\n  obj.n();\n  p->q();\n  std::max<int>(1, 2);\n}\n";
        assert_eq!(
            sites("src/s.cpp", cpp, "f"),
            ["g", "ns::h", "obj.n", "p.q", "self.m", "std::max"]
        );
        let py = "class C:\n    def f(self):\n        g()\n        self.m()\n        mod.h()\n";
        assert_eq!(sites("src/c.py", py, "f"), ["g", "mod.h", "self.m"]);
        let ts = "export class C {\n  f() {\n    g();\n    this.m();\n    obj.n();\n  }\n}\n";
        assert_eq!(sites("src/c.ts", ts, "f"), ["g", "obj.n", "self.m"]);
        let go = "package a\nfunc F() {\n\tg()\n\tpkg.H()\n}\n";
        assert_eq!(sites("src/a.go", go, "F"), ["g", "pkg.H"]);
    }
}
