//! Tree-sitter code extraction: the entry points `extract` and `extract_file`, and item collection and classification. Children: `lang` (language specs and query cache), `calls` (call sites), `signature` (signature rendering), `doc` (doc extraction).

use crate::analyze::{Def, Source};
use crate::index::Family;
use crate::model::{CodeItem, CodeItemKind, PairRole};
use crate::support::{basename, summarize, title_from_slug};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::{Node, Parser, QueryCursor, QueryMatch, StreamingIterator, Tree};

mod calls;
mod doc;
mod lang;
mod signature;

use calls::{arity_of, attach_call_sites};
pub(crate) use calls::{calls_at, import_aliases, is_function};
use doc::{item_doc_line, item_doc_text, leading_doc};
pub(crate) use doc::{plain_comment_text, plain_doc};
pub use lang::{
    ext_of, handles, is_cpp, is_cpp_fragment, is_shell, shell_shebang, source_rank,
    validate_queries, CODE_EXTENSIONS, SHEBANG_HEAD,
};
use lang::{lang_for_ext, QUERIES};
pub(crate) use lang::{LangSpec, Lead, Placement};
pub(crate) use signature::is_comment_kind;
use signature::{build_signature, shared_declaration_signature};

/// One source file's extraction: what the wiki puts on its code page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeFile {
    /// The `rel_path` given to `extract`.
    pub path: String,
    /// The language: `rust`, `python`, `cpp`, `go`, `shell`, `javascript`,
    /// `typescript`, …; `vue` or `svelte` for a component, whatever language
    /// its script is in; the extension for a file with no grammar.
    pub lang: String,
    /// The derived page name: the title-cased module stem, the directory's
    /// for a directory module (`mod.rs`, `__init__.py`, `index.*`).
    pub name: String,
    /// The text the page shows: the file's, with a Rust `#[cfg(test)]`
    /// module replaced by a one-line omission marker.
    pub body: String,
    /// The one-line summary: the file's doc, else an exported signature.
    pub summary: Option<String>,
    /// The exported signatures, grouped by owner.
    pub symbols: Vec<String>,
    /// The import strings as written, sorted and deduplicated.
    pub imports: Vec<String>,
    /// The bare names of top-level definitions, sorted and deduplicated.
    pub defined: Vec<String>,
    /// The names of exported methods (not a trait impl's), sorted and
    /// deduplicated.
    pub methods: Vec<String>,
    /// Every item with its lines, doc and call sites, in line order.
    pub items: Vec<CodeItem>,
    /// The C/C++ data members, and apart from them the enumerators, left out
    /// because there are more than `MEMBERS_PER_FILE`; 0 when every one is
    /// an item.
    pub members_unindexed: usize,
}

/// Most C/C++ data members one file indexes; past it, none are. Enumerators
/// are counted and dropped apart, under the same cap. Hand-written
/// headers measured up to 1,322 (one server's main header); a generated
/// register header holds tens of thousands.
pub const MEMBERS_PER_FILE: usize = 2_000;

/// A source file as the cross-file passes (`pair_declarations`, `Index`,
/// `Imports`) see it.
pub trait CodeUnit {
    /// The file's path relative to the project root: module stems, source
    /// ranks and `#include` suffixes are read from it.
    fn path(&self) -> &str;
    /// The code language (`CodeFile::lang`); `None` for a unit that is not code.
    fn lang(&self) -> Option<&str>;
    /// The file's items, in line order.
    fn items(&self) -> &[CodeItem];
    /// The items, for `pair_declarations` to set pairing fields on.
    fn items_mut(&mut self) -> &mut Vec<CodeItem>;
    /// The bare names of top-level definitions (`CodeFile::defined`).
    fn defined(&self) -> &[String];
}

impl CodeUnit for CodeFile {
    fn path(&self) -> &str {
        &self.path
    }
    fn lang(&self) -> Option<&str> {
        Some(&self.lang)
    }
    fn items(&self) -> &[CodeItem] {
        &self.items
    }
    fn items_mut(&mut self) -> &mut Vec<CodeItem> {
        &mut self.items
    }
    fn defined(&self) -> &[String] {
        &self.defined
    }
}

/// Extract one source file: its symbols, imports, items and summary.
pub fn extract(rel_path: &str, text: &str) -> CodeFile {
    extract_file(rel_path, text).0
}

/// The trees a file's items came from, and each item's node (parallel to
/// `items`).
pub(crate) type Found = (Vec<Source>, Vec<Option<Def>>);

/// The one extraction path behind `extract` and `analyze`: the file, and
/// what its items were built from when its extension has a grammar.
pub(crate) fn extract_file(rel_path: &str, text: &str) -> (CodeFile, Option<Found>) {
    let ext = match ext_of(rel_path) {
        e if e.is_empty() && shell_shebang(text.as_bytes()) => "sh".to_string(),
        e => e,
    };
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

    let parsed = extract_code(code_ext, code, pre_parsed);
    let grammar = parsed.is_some();
    let info = parsed.unwrap_or_else(|| CodeInfo {
        lang: ext.to_string(),
        ..CodeInfo::default()
    });
    // The original pass gates `public` on the unstripped tree, where a
    // bare `pub fn` inside the test module passes too. Exports come from
    // the stripped text, so an item is public only if the stripped pass
    // has the same item as public.
    let (mut items, sources, defs) = match original {
        Some(tree) => {
            let orig = extract_code(ext, text, Some(tree)).unwrap_or_default();
            let mut items = orig.items;
            for item in &mut items {
                item.public &= info.items.iter().any(|s| {
                    s.public
                        && s.name == item.name
                        && s.owner == item.owner
                        && s.signature == item.signature
                });
            }
            (items, orig.sources, orig.defs)
        }
        None => (info.items, info.sources, info.defs),
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
    // A component is reported as its own format (`vue`, `svelte`, which
    // is also their `lang_name`), whichever language its script is in.
    let lang_name = if script.is_some() {
        ext.to_string()
    } else {
        info.lang
    };

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

    let file = CodeFile {
        path: rel_path.to_string(),
        lang: lang_name,
        name,
        body: source,
        summary,
        symbols: info.symbols,
        imports: info.imports,
        defined: info.defined,
        methods: info.methods,
        items,
        members_unindexed: info.members_unindexed,
    };
    (file, grammar.then_some((sources, defs)))
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
    /// The trees `items` came from, and each item's node in them.
    sources: Vec<Source>,
    defs: Vec<Option<Def>>,
    /// `CodeFile::members_unindexed`.
    members_unindexed: usize,
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
/// An `or_else` chain rather than `?`: a Rust `const_item` has no
/// `declaration` field, so an early return there would skip the `value`
/// lookup and leave the initializer in the signature.
pub(crate) fn default_cut(def: Node) -> Option<Node> {
    def.child_by_field_name("body")
        .or_else(|| {
            def.child_by_field_name("declaration")
                .and_then(|d| d.child_by_field_name("body"))
        })
        .or_else(|| def.child_by_field_name("value"))
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

/// A `Rank::Value` definition cut at its `=` (or, with no `=`, at its value)
/// that re-appends the value only when there is an `=` to re-append it after.
pub(crate) fn value_shape<'a>(
    eq: Option<Node<'a>>,
    value: Option<Node<'a>>,
    has_type: bool,
) -> Shape<'a> {
    Shape {
        rank: Rank::Value,
        cut: eq.or(value),
        value: eq.and(value),
        has_type,
    }
}

/// Climbs from `node` while the parent's kind satisfies `up`; the topmost such
/// ancestor, or `node` itself when the parent does not.
pub(crate) fn outermost(mut node: Node<'_>, up: impl Fn(&str) -> bool) -> Node<'_> {
    while let Some(parent) = node.parent().filter(|p| up(p.kind())) {
        node = parent;
    }
    node
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
    // A wrapped C/C++ definition's `@name` is its whole declarator: walk
    // it to the function's name, and drop the match when it names none.
    if let (Some(def), Some(d)) = (parts.def, parts.name) {
        if def.kind() == "function_definition" && d.kind().ends_with("_declarator") {
            parts.name = super::extract_cpp::defined_name(d);
            parts.def = parts.name.and(parts.def);
        }
    }
    // A C/C++ data member's `@name` is its whole declarator: walk it to the
    // name, and to none for a declarator that is no data member.
    if let (Some(item), Some(d)) = (parts.item, parts.name) {
        if item.kind() == "field_declaration" {
            parts.name = super::extract_cpp::is_data_declarator(d)
                .then(|| super::extract_cpp::declared_name(d))
                .flatten();
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
    owner
        .or(name_text)
        .map(str::to_string)
        .or_else(|| (spec.header_group)(def, text))
        .unwrap_or_default()
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
    // Compiled once per process. A malformed query must not silently strip a
    // language of its symbols while the compile exits 0 — `QUERIES` panics on
    // a bad query, and `validate_queries` forces that before any output.
    let query = QUERIES.get(&spec.language)?;
    // A C/C++ tree of the first preprocessor branch: its items come first,
    // the original tree adds what it lacks (`#else` definitions).
    let mut first_branch = None;
    let tree = if let Some(tree) = pre_parsed {
        tree
    } else {
        let mut parser = Parser::new();
        parser.set_language(&spec.language).ok()?;
        let tree = parser.parse(text, None)?;
        match spec.lang_name {
            "shell" => super::extract_shell::reparse(&mut parser, text, tree),
            "cpp" => {
                first_branch = super::extract_cpp::reparse(&mut parser, text, &tree);
                tree
            }
            _ => tree,
        }
    };
    // The first-branch tree comes first, as its items do, with the copy it
    // was parsed from: same length and lines as `text`, other branches blank.
    let family = Family::of(Some(spec.lang_name));
    let sources: Vec<Source> = first_branch
        .into_iter()
        .chain([(tree, text.to_string())])
        .map(|(t, s)| Source::new(t, s, family))
        .collect();
    let main = sources.len() - 1;
    let tree = sources[main].tree();
    let idx = CaptureIdx {
        def: query.capture_index_for_name("def"),
        name: query.capture_index_for_name("name"),
        vis: query.capture_index_for_name("vis"),
        imp: query.capture_index_for_name("import"),
        item: query.capture_index_for_name("item"),
    };
    let exports = (spec.export_set)(tree, text);

    // (group, kind, name, signature). The sort key groups a definition with
    // its own members: `class Article` and `Article.title: str` share the
    // group `Article`, so a class does not scatter across the section
    // because `@` < `A` < `d`. Only the ORDER changes — `CodeFile::symbols`
    // stays a plain `Vec<String>`.
    let mut collected: Vec<Collected> = Vec::new();
    // Method names for `CodeFile::methods`. Gathered here, not as a second
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
    let mut items: BTreeMap<usize, (CodeItem, Option<Def>)> = BTreeMap::new();
    // C/C++ data member declarators seen; past `MEMBERS_PER_FILE` none is
    // built, since none will be kept.
    let mut members = 0;
    // Enumerators, counted apart, and their items' keys.
    let mut enumerators = 0;
    let mut enumerator_keys = BTreeSet::new();
    for (source, t) in sources.iter().enumerate() {
        let text = t.text();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, t.tree().root_node(), text.as_bytes());
        while let Some(m) = matches.next() {
            let parts = split_captures(m, &idx, text, &mut imports);
            if parts.name.is_some() && parts.item.is_some_and(|i| i.kind() == "field_declaration") {
                members += 1;
                if members > MEMBERS_PER_FILE {
                    continue;
                }
            }
            if let Some(n) = parts.name.filter(|_| parts.item.is_some_and(is_enum_value)) {
                enumerators += 1;
                enumerator_keys.insert(n.start_byte());
                if enumerators > MEMBERS_PER_FILE {
                    continue;
                }
            }
            let Some(def) = parts.def.or(parts.item) else {
                continue;
            };
            let (chain, nested) = match (spec.placement)(def, text) {
                Placement::Scoped(chain) => (chain, false),
                Placement::Nested => (Vec::new(), true),
                Placement::Rejected => continue,
            };
            let name_text = parts.name.and_then(|n| name_text(n, text));
            let shape = (spec.shape)(def);
            let owner = (!chain.is_empty()).then(|| chain.join(spec.owner_sep));
            let public = if nested {
                false
            } else if parts.def.is_some() {
                let keep = should_keep(parts.vis, &chain, name_text, exports.as_ref(), &spec);
                if keep {
                    // Resolved once, after `placement` has accepted the item, and
                    // then used for both the signature and the classification, so
                    // the node's kind is derived once.
                    // A misread conversion operator already reads `K::operator
                    // bool` in the source: nothing for the owner to replace.
                    let misread = parts
                        .name
                        .is_some_and(|n| super::extract_cpp::misread_cast(n, text).is_some());
                    let spliced = owner.as_deref().filter(|_| !misread);
                    let sig = build_signature(text, def, parts.name, spliced, &spec, &shape);
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
                keep
            } else {
                !spec.query_gates_visibility
                    && should_keep(None, &chain, name_text, exports.as_ref(), &spec)
            };
            let item = code_item(
                def,
                parts.name,
                owner.as_deref(),
                public,
                text,
                &spec,
                &shape,
            );
            let item = item.map(|(key, mut it, node)| {
                it.nested |= nested;
                let def = Def {
                    source,
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                };
                (key, (it, Some(def)))
            });
            merge_item(&mut items, item);
        }
    }
    // Data members (C/C++ only) and enum values are nested values; each
    // kind past the cap keeps none.
    let members = if spec.lang_name == "cpp" { members } else { 0 };
    let over = |n: usize| n > MEMBERS_PER_FILE;
    if over(members) || over(enumerators) {
        items.retain(|key, (i, _)| {
            let counted = if enumerator_keys.contains(key) {
                enumerators
            } else {
                members
            };
            !(i.nested && i.kind == CodeItemKind::Value && over(counted))
        });
    }
    let members_unindexed = [members, enumerators]
        .into_iter()
        .filter(|&n| over(n))
        .sum();
    let (mut items, mut defs): (Vec<CodeItem>, Vec<Option<Def>>) = items.into_values().unzip();
    sort_items(&mut items, &mut defs);
    if spec.lang_name == "shell" && tree.root_node().has_error() {
        let missed = super::extract_shell::missed_functions(text, &items);
        for it in &missed {
            let (n, sig) = (it.name.clone(), it.signature.clone());
            collected.push((n.clone(), ItemKind::FreeDef, n, sig));
        }
        defs.resize(defs.len() + missed.len(), None);
        items.extend(missed);
    }
    if spec.lang_name == "shell" {
        let globals = super::extract_shell::globals(tree.root_node(), text, &items);
        for (item, stmt) in globals {
            items.push(item);
            defs.push(Some(Def {
                source: main,
                start_byte: stmt.start_byte(),
                end_byte: stmt.end_byte(),
            }));
        }
        sort_items(&mut items, &mut defs);
    }
    attach_call_sites(&mut items, &sources[main]);
    // A shell file's `<top level>` item, pushed last, has no single node.
    defs.resize(items.len(), None);
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
        super::extract_python::docstring_paragraph(tree.root_node(), text)
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
        sources,
        defs,
        members_unindexed,
    })
}

/// Items in (start line, end line, name) order, each def kept with its item.
/// The sort is stable, as the one over the items alone was.
fn sort_items(items: &mut Vec<CodeItem>, defs: &mut Vec<Option<Def>>) {
    let mut both: Vec<(CodeItem, Option<Def>)> = items.drain(..).zip(defs.drain(..)).collect();
    both.sort_by(|(a, _), (b, _)| {
        (a.start_line, a.end_line, &a.name).cmp(&(b.start_line, b.end_line, &b.name))
    });
    (*items, *defs) = both.into_iter().unzip();
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

/// The text an `@name` node names. A C++ conversion operator has no node
/// for its name alone: its `@name` is the whole `operator_cast`
/// (`operator Kind*() const`), and the name is what precedes the function
/// declarator nested in it (`operator Kind*`). `None` for a cast with no
/// function declarator, which names nothing.
fn name_text<'a>(name: Node, text: &'a str) -> Option<&'a str> {
    // A misread one's name runs from its `operator` to the type it names.
    if let Some(operator) = super::extract_cpp::misread_cast(name, text) {
        let end = outermost(name, |k| k == "qualified_identifier").end_byte();
        return text.get(operator.start_byte()..end);
    }
    if name.kind() != "operator_cast" {
        return text.get(name.byte_range());
    }
    let params = std::iter::successors(name.child_by_field_name("declarator"), |d| {
        d.named_child(u32::try_from(d.named_child_count()).ok()?.checked_sub(1)?)
    })
    .find(|d| d.kind() == "abstract_function_declarator")?;
    text.get(name.start_byte()..params.start_byte())
        .map(str::trim_end)
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
/// kind (a field outside C/C++, a variant, an associated const), or an
/// empty signature.
///
/// The range climbs from the signature's start node over the contiguous
/// leads above it; a blank line, a lead trailing other code on its line, or
/// a `Stop` lead (other code) ends the climb, so a stray doc comment never
/// stretches the range. A plain block directly above the item (or its
/// attributes) extends the range only when `plain_doc` makes it the doc.
fn code_item<'t>(
    def: Node<'t>,
    name: Option<Node>,
    owner: Option<&str>,
    public: bool,
    text: &str,
    spec: &LangSpec,
    shape: &Shape,
) -> Option<(usize, CodeItem, Node<'t>)> {
    let name = name?;
    // `V [[deprecated]] = 4` and `V __attribute__((deprecated)) = 4` can
    // parse as an error holding `V [[` (`V __attribute__((`) before the value
    // `deprecated]] = 4`: the value is that `V`, and reads from there. Any
    // other error just before an enum value means its name is a word the
    // grammar lost its place in (an attribute's, a macro argument): none.
    let after_error = Some(def)
        .filter(|d| is_enum_value(*d))
        .and_then(|d| d.prev_sibling())
        .filter(Node::is_error);
    let attributed = after_error.and_then(|e| {
        let v = e.named_child(0).filter(|n| n.kind() == "identifier")?;
        let rest = text[v.end_byte()..e.end_byte()].trim_start();
        (rest.starts_with("[[") || rest.starts_with("__attribute__")).then_some((e, v))
    });
    if after_error.is_some() && attributed.is_none() {
        return None;
    }
    let name = attributed.map_or(name, |(_, n)| n);
    // JS exports match on the `export_statement`, which is no function: the
    // item is its declaration. `sig_start` climbs back to the statement, so
    // the range and signature are the declaration's own match's.
    let def = def
        .child_by_field_name("declaration")
        .filter(|_| def.kind() == "export_statement")
        .unwrap_or(def);
    // A C/C++ data member or an enum's value: a value `search` finds by its
    // exact name only.
    let member = is_enum_value(def)
        || (spec.lang_name == "cpp" && super::extract_cpp::names_data_member(def, name));
    // An error after a data member's name means the grammar misread it:
    // `int Box::* pm;` names `Box`, `int *RESTRICT slots;` names `RESTRICT`.
    // One before it (`ATOMIC int ticks;`, a macro the grammar cannot know)
    // leaves it right. An enum value's name always comes first, so an error
    // after it is an attribute the grammar cannot read (`V ATTR = 4`); but
    // one nothing well-formed follows (no `,` or `}`) is a macro call the
    // grammar misread (`ENTRY(x)` in an X-macro enum), unless a standard
    // attribute follows its name (`V [[deprecated]] = 4` ends the enum).
    let misread = if is_enum_value(def) {
        let attribute = text[name.end_byte()..].trim_start().starts_with("[[");
        !attribute
            && def
                .next_sibling()
                .is_none_or(|s| s.is_missing() || s.is_error())
    } else {
        member
            && def
                .children(&mut def.walk())
                .any(|c| c.is_error() && c.start_byte() >= name.end_byte())
    };
    if misread {
        return None;
    }
    let kind = match (classify(Some(name), owner, shape.rank), is_function(def)) {
        _ if member => CodeItemKind::Value,
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
    let signature = match attributed {
        Some((e, _)) => text[e.start_byte()..def.end_byte()]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        None => shared_declaration_signature(text, def, name, spec)
            .unwrap_or_else(|| build_signature(text, def, Some(name), None, spec, shape)),
    };
    if signature.is_empty() {
        return None;
    }

    let name_text = name_text(name, text)?;
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
        // A `Stop` ends the walk below whatever else the group holds, so the
        // group stops growing at one: a line of thousands of members would
        // otherwise be walked back once per item.
        while !opens_its_line(group[group.len() - 1], text)
            && !matches!((spec.item_lead)(group[group.len() - 1], text), Lead::Stop)
        {
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
        nested: member,
        kind,
        name: name_text.to_string(),
        owner: owner.map(str::to_string),
        signature,
        start_line: first.start_position().row + 1,
        end_line: last_row(top) + 1,
        doc: raw_doc.as_deref().and_then(item_doc_line),
        doc_text: None,
        public: public && !member,
        twin: None,
        test: false,
        calls: Vec::new(),
        call_lines: Vec::new(),
        call_sites: Vec::new(),
        arity: match kind {
            CodeItemKind::Fn | CodeItemKind::Method => {
                arity_of(def, spec.lang_name, kind == CodeItemKind::Method, text)
            }
            _ => None,
        },
        pair_role: if spec.lang_name == "cpp" {
            super::extract_cpp::pair_role(def, text)
        } else {
            PairRole::Neither
        },
        namespace: if spec.lang_name == "cpp" {
            super::extract_cpp::namespace_of(def, text)
        } else {
            None
        },
        params: match kind {
            CodeItemKind::Fn | CodeItemKind::Method if spec.lang_name == "cpp" => {
                super::extract_cpp::params_of(def, name_text, text)
            }
            _ => None,
        },
    };
    item.doc_text = raw_doc
        .as_deref()
        .and_then(|r| item_doc_text(r, item.doc.as_deref()));
    if let Some((suite, test)) = (spec.lang_name == "cpp")
        .then(|| super::extract_cpp::gtest_names(def, text))
        .flatten()
    {
        item.name = test.to_string();
        item.owner = Some(suite.to_string());
    }
    Some((name.start_byte(), item, def))
}

/// An enum's value: a C/C++ enumerator, a Rust variant, a TS enum member
/// (bare or assigned).
fn is_enum_value(def: Node) -> bool {
    matches!(
        def.kind(),
        "enumerator" | "enum_variant" | "enum_assignment"
    ) || (def.kind() == "property_identifier"
        && def.parent().is_some_and(|p| p.kind() == "enum_body"))
}

/// Keep the first item seen for a name node. A later match of the same node
/// only ORs in its `public` flag: of a `pub fn`'s two matches, the @item one
/// alone says private.
fn merge_item(
    items: &mut BTreeMap<usize, (CodeItem, Option<Def>)>,
    found: Option<(usize, (CodeItem, Option<Def>))>,
) {
    if let Some((key, item)) = found {
        let public = item.0.public;
        items
            .entry(key)
            .and_modify(|kept| kept.0.public |= public)
            .or_insert(item);
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
/// `index.mjs` / `index.cjs` likewise.
const DIRECTORY_MODULES: &[(&str, &str)] = &[
    ("rs", "mod"),
    ("py", "__init__"),
    ("ts", "index"),
    ("js", "index"),
    ("mjs", "index"),
    ("cjs", "index"),
    ("tsx", "index"),
    ("jsx", "index"),
];

/// The identifier a code file's module is known by, before any casing: the
/// base name up to its first `.`, or the parent directory's name for a
/// directory-module file (`mod.rs`, `__init__.py`) that has a parent. Shared
/// by page naming (`derive_code_name`) and by `graph.rs` (import resolution
/// and the code-shaped mention filter), so there is exactly one definition
/// of how a code page is named.
pub fn module_stem(rel_path: &str) -> &str {
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
/// declaration or a `common::` path link it, and `imports::Imports`
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

    /// The `(doc, start_line)` of the item named `name` in `src`.
    pub(crate) fn doc_of(path: &str, src: &str, name: &str) -> (Option<String>, usize) {
        let e = crate::extract(path, src);
        let item = e.items.iter().find(|i| i.name == name).unwrap();
        (item.doc.clone(), item.start_line)
    }

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
mod tests;
