//! Per-language specs: the `LangSpec` table, extension lookup and source ranking, and the process-wide tree-sitter query cache.

use super::{default_shape, Shape};
use crate::support::basename;
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Query, Tree};

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
    /// What a captured definition is and where its signature stops. A plain-JS
    /// Vue or Svelte script uses `default_shape`; a `lang` script uses that
    /// language's own spec.
    pub(crate) shape: fn(Node) -> Shape,
    /// A signature the language writes itself (shell: `name()` whatever the
    /// definition's spelling), given the definition and its name. `None`
    /// builds it from the source.
    pub(crate) fixed_signature: fn(Node, &str) -> Option<String>,
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
            fixed_signature: |_, _| None,
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
/// One answer for both "is it public" and "who owns it": Python derives both
/// from a single upward traversal of the tree, so they are not split.
pub(crate) enum Placement {
    /// Not part of the module's surface — drop the definition outright.
    Rejected,
    /// Enclosing scope names, outermost first. Empty means a free item whose
    /// signature is used verbatim.
    Scoped(Vec<String>),
    /// A function defined directly in a function body (Python): an item for
    /// `search`, never an export, an outline line or a call-graph node.
    Nested,
}

pub(super) fn lang_for_ext(ext: &str) -> Option<LangSpec> {
    Some(match ext {
        "rs" => super::extract_rust::rust_spec(),
        "py" => super::extract_python::python_spec(),
        "js" | "mjs" | "cjs" => super::extract_simple::js_spec(),
        "ts" => super::extract_simple::ts_spec(),
        "jsx" => super::extract_jsx::jsx_spec(),
        "tsx" => super::extract_jsx::tsx_spec(),
        "vue" => super::extract_vue::vue_spec(),
        "svelte" => super::extract_svelte::svelte_spec(),
        "go" => super::extract_simple::go_spec(),
        "sh" | "bash" => super::extract_shell::shell_spec(),
        _ if is_cpp(ext) => super::extract_cpp::cpp_spec(),
        _ => return None,
    })
}

/// The extensions the C++ spec claims; `CODE_EXTENSIONS` lists them too.
pub fn is_cpp(ext: &str) -> bool {
    matches!(ext, "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "h" | "c")
}

/// The lowercased extension of `rel_path`, or `""` when the basename has no
/// extension (`Makefile`) or is a dotfile (`.gitignore`).
///
/// Lowercasing matters: `handles` and the grammar lookup match the lowercase
/// form, so `Book.H` must reach `lang_for_ext` as `h`, not fall through to
/// zero symbols.
pub fn ext_of(rel_path: &str) -> String {
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
/// unit implementing either; a shell script ranks last, so it never takes the
/// name from a same-named Python or C++ page. A TOTAL order, not a binary: with every header at
/// one rank, adding `foo.hpp` beside `foo.h` would tie at the top, dissolve
/// the strict win in `disambiguate_ids`, and rename the pre-existing page.
pub fn source_rank(source_path: &str) -> u8 {
    match ext_of(source_path).as_str() {
        "h" => 0,
        "hpp" | "hh" | "hxx" => 1,
        "sh" | "bash" => 3,
        _ => 2,
    }
}

/// A shell script: reached only by its path (`source`), never by a module
/// stem or a defined name, so `import deploy` in Python cannot link `deploy.sh`.
pub fn is_shell(source_path: &str) -> bool {
    matches!(ext_of(source_path).as_str(), "sh" | "bash")
}

/// Every extension `extract` has a grammar for (`handles` checks it). A test
/// pins it to the extensions `QUERIES` builds a query for, so the two cannot
/// drift: a missing query would strip that language of all symbols and imports.
pub const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "vue", "svelte", "go", "cpp", "cc", "cxx", "hpp", "hh",
    "hxx", "h", "c", "mjs", "cjs", "sh", "bash",
];

/// Each grammar's query, compiled once per process instead of once per file,
/// and shared by every extension on that grammar (seven for C++).
///
/// Keyed by the grammar, not by `lang_name`: `ts` and `tsx` are both
/// "typescript" on two grammars, and a query only runs on the grammar it was
/// built for. One grammar has one query string; a test pins that.
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
pub(super) static QUERIES: LazyLock<HashMap<Language, Query>> = LazyLock::new(|| {
    let mut queries = HashMap::new();
    for ext in CODE_EXTENSIONS {
        let spec = lang_for_ext(ext)
            .unwrap_or_else(|| panic!("no LangSpec registered for extension {ext:?}"));
        if let Entry::Vacant(slot) = queries.entry(spec.language) {
            let query = Query::new(slot.key(), spec.query_src).unwrap_or_else(|e| {
                panic!("invalid tree-sitter query for {}: {e:?}", spec.lang_name)
            });
            slot.insert(query);
        }
    }
    queries
});

/// Force every language query to compile.
///
/// Called at the top of a compile so a malformed query fails loudly and
/// deterministically *before* any page is written. Without this the first
/// failure would surface lazily inside a rayon worker, partway through a
/// parallel run, with output already committed for other files.
pub fn validate_queries() {
    LazyLock::force(&QUERIES);
    super::extract_python::validate_queries();
    super::extract_rust::validate_queries();
}

/// Whether `extract` claims `rel_path`: its lowercased extension is in
/// `CODE_EXTENSIONS`. A file whose name is only an extension (`.rs`) has none
/// (see `ext_of`) and is not handled.
pub fn handles(rel_path: &str) -> bool {
    CODE_EXTENSIONS.contains(&ext_of(rel_path).as_str())
}
