//! Tree-sitter code extraction: per-language specs, the process-wide query cache, and the signature pipeline behind `## Exports` and `## Imports`.

use crate::formats::{summarize, Extractor};
use crate::model::{slugify, title_case, Entity, SourceKind};
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
    /// The group a `@def` with no `@name` shares with its own members — only
    /// Rust's trait-impl header pattern needs this; every other language
    /// resolves a group from `owner` or `@name` and never reaches it.
    pub(crate) header_group: fn(Node, &str) -> Option<String>,
}

pub(crate) fn sig_start_identity(def: Node) -> Node {
    def
}

pub(crate) fn keep_all(_name: &str) -> bool {
    true
}

pub(crate) fn keep_any_vis(_vis: &str) -> bool {
    true
}

pub(crate) fn no_export_set(_tree: &Tree, _text: &str) -> Option<BTreeSet<String>> {
    None
}

/// Only Rust emits a `@def` without a `@name` (the trait-impl header pattern),
/// so every other language never reaches this.
pub(crate) fn no_header_group(_def: Node, _text: &str) -> Option<String> {
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

pub(crate) fn always_free(_def: Node, _text: &str) -> Placement {
    Placement::Scoped(Vec::new())
}

fn lang_for_ext(ext: &str) -> Option<LangSpec> {
    Some(match ext {
        "rs" => super::extract_rust::rust_spec(),
        "py" => super::extract_python::python_spec(),
        "js" => super::extract_simple::js_spec(),
        "ts" => super::extract_simple::ts_spec(),
        "go" => super::extract_simple::go_spec(),
        _ => return None,
    })
}

/// The per-language `Shape` hook. Kept beside `lang_for_ext` rather than as a
/// `LangSpec` field so that adding this seam needed no edit to the three
/// unfinished JS/TS/Go specs, which spell every field out literally. If those
/// languages ever grow grammar-specific shapes, this folds into `LangSpec`.
///
/// Everything this dispatches to lives in the language's own module: no Rust
/// or Python node kind appears in this file.
fn shape_for(ext: &str) -> fn(Node) -> Shape {
    match ext {
        "rs" => super::extract_rust::rust_shape,
        "py" => super::extract_python::python_shape,
        _ => default_shape,
    }
}

/// Every extension `CodeExtractor` claims. Shared by `extensions()` and by
/// `QUERIES` so the registry can never miss a language the extractor accepts —
/// a missing entry would strip that language of all symbols and imports.
const CODE_EXTENSIONS: &[&str] = &["rs", "py", "js", "ts", "go"];

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
        let ext = rel_path.rsplit('.').next().unwrap_or("");

        // Test modules are orientation noise that inflate the page body, its
        // token_estimate, and the neighbors budget (dogfood finding #12) —
        // splice them out, leaving an honest omission marker. Stripping runs
        // BEFORE extraction (finding #13) so the body, the symbols, the
        // imports, and the doc comment all describe the same source.
        // When stripping finds nothing it hands back the tree it already
        // parsed, and extraction reuses it rather than parsing the same bytes
        // again. A file that IS stripped must reparse: the text changed.
        let (source, pre_parsed) = match (ext == "rs")
            .then(|| super::extract_rust::strip_rust_test_modules(text))
            .flatten()
        {
            Some(super::extract_rust::Stripped::Unchanged(tree)) => (text.to_string(), Some(tree)),
            Some(super::extract_rust::Stripped::Rewritten(out)) => (out, None),
            None => (text.to_string(), None),
        };

        let (lang_name, symbols, imports, defined, methods, docstring, summary_fallback) =
            match extract_code(ext, &source, pre_parsed) {
                Some(v) => v,
                None => (
                    ext.to_string(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    None,
                    None,
                ),
            };

        let docstring = docstring.or_else(|| leading_doc(&source, ext));
        // `body` is deliberately NOT scanned for a summary: source code is not
        // prose, so letting summarize() hunt line-by-line for a "real
        // sentence" produces garbage. Only a real docstring, or (failing
        // that) an exported signature, is an acceptable summary.
        let summary = summarize(None, docstring.as_deref(), "", summary_fallback.as_deref());
        let name = derive_code_name(rel_path);

        Entity {
            id: slugify(&name),
            name,
            aliases: Vec::new(),
            created: String::new(),
            body: source,
            source_path: String::new(),
            kind: SourceKind::Code { lang: lang_name },
            content_hash: [0u8; 32],
            summary,
            symbols,
            imports,
            defined,
            methods,
        }
    }
}

/// `(language, symbols, imports, defined, methods, docstring,
/// summary_fallback)`. The fallback is chosen here rather than by the
/// caller: freeness is known only while the captures are in hand, and
/// `symbols` alone cannot be reinterpreted after sorting.
type CodeInfo = (
    String,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Option<String>,
    Option<String>,
);

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
}

/// The three single-valued captures one query match can contribute: the
/// definition and name nodes, plus the visibility keyword's text. Imports are
/// deliberately absent: a single match may carry several, so they go straight
/// into the caller's list rather than through this struct.
struct MatchParts<'a> {
    def: Option<Node<'a>>,
    name: Option<Node<'a>>,
    vis: Option<&'a str>,
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
    };
    for cap in m.captures {
        if Some(cap.index) == idx.def {
            parts.def = Some(cap.node);
        } else if Some(cap.index) == idx.name {
            parts.name = Some(cap.node);
        } else if Some(cap.index) == idx.vis {
            parts.vis = text.get(cap.node.byte_range());
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
    };
    let exports = (spec.export_set)(&tree, text);
    let shape_of = shape_for(ext);

    // (group, kind, name, signature). The sort key groups a definition with
    // its own members: `class Article` and `Article.title: str` share the
    // group `Article`, so a class no longer scatters across the section
    // because `@` < `A` < `d`. Only the ORDER changes — `Entity::symbols`
    // stays a plain `Vec<String>`.
    let mut collected: Vec<Collected> = Vec::new();
    // Method names for `Entity::methods`. Gathered here, not as a second
    // projection over `collected`: that tuple carries no node kind, and a
    // `fn`, a field and an associated `const` of one impl are all `Member`,
    // told apart only by signature text, which is never parsed. The three
    // node kinds are the seam's one concession: they name "a function" in
    // the grammars this file already dispatches on.
    let mut methods: Vec<String> = Vec::new();
    let mut imports = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        let parts = split_captures(m, &idx, text, &mut imports);
        if let Some(def) = parts.def {
            let Placement::Scoped(chain) = (spec.placement)(def, text) else {
                continue;
            };
            let name_text = parts.name.and_then(|n| text.get(n.byte_range()));
            if should_keep(parts.vis, &chain, name_text, exports.as_ref(), &spec) {
                let owner = (!chain.is_empty()).then(|| chain.join(spec.owner_sep));
                // Resolved once, after `placement` has accepted the item, and
                // then used for both the signature and the classification —
                // the two used to re-derive the node's kind independently.
                let shape = shape_of(def);
                let sig = build_signature(text, def, parts.name, owner.as_deref(), &spec, &shape);
                if !sig.is_empty() {
                    let kind = classify(parts.name, owner.as_deref(), shape.rank);
                    let group = group_key(owner.as_deref(), name_text, def, text, &spec);
                    let is_function = matches!(
                        def.kind(),
                        "function_item" | "function_signature_item" | "function_definition"
                    );
                    let trait_impl = owner
                        .as_deref()
                        .is_some_and(super::extract_rust::is_trait_impl);
                    if kind == ItemKind::Member && is_function && !trait_impl {
                        if let Some(n) = name_text {
                            methods.push(n.to_string());
                        }
                    }
                    collected.push((group, kind, name_text.unwrap_or("").to_string(), sig));
                }
            }
        }
    }
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

    Some((
        spec.lang_name.to_string(),
        symbols,
        imports,
        defined,
        methods,
        docstring,
        summary_fallback,
    ))
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
/// name node's start byte, so it renders two fragments and concatenates them;
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

/// Build a one-line signature from a definition node: its source text up to
/// (not including) its `cut` node, with internal whitespace collapsed,
/// punctuation tidied, and language-specific trailing characters (Rust's `;`
/// and `=`, Python's `:`) removed.
///
/// When an `owner` is known, it is spliced in at the name node's start byte —
/// a byte-range operation, so there is no substring search to mismatch.
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
    let raw = match (owner, name) {
        (Some(owner), Some(name)) if name.start_byte() >= start && name.start_byte() <= end => {
            format!(
                "{}{}{}{}",
                render_span(text, root, start, name.start_byte(), spec),
                owner,
                spec.owner_sep,
                render_span(text, root, name.start_byte(), end, spec)
            )
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

/// Module-level doc comment, one line, if present. This is the fallback for
/// languages without an AST-verified docstring convention (everything but
/// Python, where `python_docstring` is used instead).
fn leading_doc(text: &str, ext: &str) -> Option<String> {
    let strip = |line: &str| -> Option<String> {
        let doc = match ext {
            "rs" => line
                .strip_prefix("//!")
                .or_else(|| line.strip_prefix("///")),
            "py" => line
                .strip_prefix("\"\"\"")
                .map(|s| s.trim_end_matches("\"\"\"")),
            "js" | "ts" | "go" => line.strip_prefix("//").or_else(|| line.strip_prefix("/*")),
            _ => None,
        };
        doc.map(|d| d.trim().to_string())
    };
    let mut lines = text.lines().map(str::trim).skip_while(|l| l.is_empty());
    let first = strip(lines.next()?).filter(|d| !d.is_empty())?;
    // The whole opening paragraph, not just its first line: `summarize` cuts
    // the sentence, and a `//!` sentence is routinely wrapped at 80 columns.
    let rest = lines.map_while(|l| strip(l).filter(|d| !d.is_empty()));
    Some(
        std::iter::once(first)
            .chain(rest)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Filenames that say nothing about the module's own identity: the language
/// names such a module after its DIRECTORY.
///
/// `index.js` / `index.ts` is the same shape for JS and TS. Left out while
/// those languages are unfinished rather than added blind.
const DIRECTORY_MODULES: &[(&str, &str)] = &[("rs", "mod"), ("py", "__init__")];

/// The identifier a code file's module is known by, before any casing: the
/// base name up to its first `.`, or the parent directory's name for a
/// directory-module file (`mod.rs`, `__init__.py`) that has a parent. Shared
/// by page naming (`derive_code_name`) and by `graph.rs` (import resolution
/// and the code-shaped mention filter), so there is exactly one definition
/// of how a code page is named.
pub(crate) fn module_stem(rel_path: &str) -> &str {
    let base = rel_path.rsplit('/').next().unwrap_or(rel_path);
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
/// declaration or a `common::` path link it, and `graph::resolve_import`
/// reaches it through the same stem.
fn derive_code_name(rel_path: &str) -> String {
    title_case(&module_stem(rel_path).replace(['_', '-'], " "))
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
