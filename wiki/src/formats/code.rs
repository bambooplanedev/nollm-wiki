use crate::formats::{summarize, Extractor};
use crate::model::{slugify, Entity, SourceKind};
use std::collections::BTreeSet;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, QueryMatch, Tree};

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

impl Extractor for CodeExtractor {
    fn extensions(&self) -> &[&str] {
        &["rs", "py", "js", "ts", "go"]
    }

    fn extract(&self, rel_path: &str, text: &str) -> Entity {
        let ext = rel_path.rsplit('.').next().unwrap_or("");

        // Test modules are orientation noise that inflate the page body, its
        // token_estimate, and the neighbors budget (dogfood finding #12) —
        // splice them out, leaving an honest omission marker. Stripping runs
        // BEFORE extraction (finding #13) so the body, the symbols, the
        // imports, and the doc comment all describe the same source.
        let source = if ext == "rs" {
            super::extract_rust::strip_rust_test_modules(text).unwrap_or_else(|| text.to_string())
        } else {
            text.to_string()
        };

        let (lang_name, symbols, imports, docstring, summary_fallback) =
            match extract_code(ext, &source) {
                Some(v) => v,
                None => (ext.to_string(), Vec::new(), Vec::new(), None, None),
            };

        let docstring = docstring.or_else(|| leading_doc(&source, ext));
        // `body` is deliberately NOT scanned for a summary: source code is not
        // prose, so letting summarize() hunt line-by-line for a "real
        // sentence" produces garbage. Only a real docstring, or (failing
        // that) an exported signature, is an acceptable summary.
        let summary = summarize(None, docstring.as_deref(), "", summary_fallback.as_deref());
        let name = derive_name_from_path(rel_path);

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
        }
    }
}

/// `(language, symbols, imports, docstring, summary_fallback)`. The fallback
/// is chosen here rather than by the caller: freeness is known only while the
/// captures are in hand, and `symbols` alone cannot be reinterpreted after
/// sorting.
type CodeInfo = (
    String,
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
    /// A method, field, or associated item.
    Member,
}

/// A definition whose name binds a value rather than declaring a callable or a
/// type. Consulted only for items `placement` already reported as free — an
/// associated `const` inside a trait impl is a `Member`, not a `FreeValue`.
fn is_value_item(kind: &str) -> bool {
    matches!(
        kind,
        "assignment" | "const_item" | "static_item" | "type_item"
    )
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
    exports: &Option<BTreeSet<String>>,
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
        name_text.map(|n| (spec.name_filter)(n)).unwrap_or(true)
    };
    root_ok
        && own_name_ok
        && chain.iter().skip(1).all(|c| (spec.name_filter)(c))
        && vis.map(|v| (spec.vis_filter)(v)).unwrap_or(true)
}

/// The shape of a kept definition, decided from the two `Option`s already in
/// hand plus the node's own kind, rather than sniffed back out of the
/// rendered signature.
fn classify(name_node: Option<Node>, owner: Option<&str>, def: Node) -> ItemKind {
    if name_node.is_none() {
        ItemKind::Header
    } else if owner.is_some() {
        ItemKind::Member
    } else if is_value_item(def.kind()) {
        ItemKind::FreeValue
    } else {
        ItemKind::FreeDef
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
        .or_else(|| collected.iter().min_by(|a, b| a.3.cmp(&b.3)))
        .map(|(_, _, _, sig)| sig.clone())
}

fn extract_code(ext: &str, text: &str) -> Option<CodeInfo> {
    let spec = lang_for_ext(ext)?;
    let mut parser = Parser::new();
    parser.set_language(&spec.language).ok()?;
    let tree = parser.parse(text, None)?;
    let query = match Query::new(&spec.language, spec.query_src) {
        Ok(q) => q,
        Err(e) => {
            // Silently returning None here strips every page of this language
            // of all symbols and all imports while the compile still exits 0.
            debug_assert!(false, "invalid {} query: {e:?}", spec.lang_name);
            eprintln!(
                "warning: invalid tree-sitter query for {} — pages of this language will have no symbols or imports: {e:?}",
                spec.lang_name
            );
            return None;
        }
    };
    let idx = CaptureIdx {
        def: query.capture_index_for_name("def"),
        name: query.capture_index_for_name("name"),
        vis: query.capture_index_for_name("vis"),
        imp: query.capture_index_for_name("import"),
    };
    let exports = (spec.export_set)(&tree, text);

    // (group, kind, name, signature). The sort key groups a definition with
    // its own members: `class Article` and `Article.title: str` share the
    // group `Article`, so a class no longer scatters across the section
    // because `@` < `A` < `d`. Only the ORDER changes — `Entity::symbols`
    // stays a plain `Vec<String>`.
    let mut collected: Vec<Collected> = Vec::new();
    let mut imports = Vec::new();
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    for m in matches {
        let parts = split_captures(&m, &idx, text, &mut imports);
        if let Some(def) = parts.def {
            let Placement::Scoped(chain) = (spec.placement)(def, text) else {
                continue;
            };
            let name_text = parts.name.and_then(|n| text.get(n.byte_range()));
            if should_keep(parts.vis, &chain, name_text, &exports, &spec) {
                let owner = (!chain.is_empty()).then(|| chain.join(spec.owner_sep));
                let sig = build_signature(
                    text,
                    def,
                    parts.name,
                    owner.as_deref(),
                    &spec,
                    signature_cut(def),
                );
                if !sig.is_empty() {
                    let kind = classify(parts.name, owner.as_deref(), def);
                    let group = group_key(owner.as_deref(), name_text, def, text, &spec);
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
        docstring,
        summary_fallback,
    ))
}

/// Locate the node at which a definition's signature stops, so the rest of its
/// text can be excluded. Most grammars expose this as a `body` field; JS/TS
/// definitions captured through an `export_statement` wrapper expose it one
/// level down via the wrapped `declaration`; Rust `const`/`static` items have
/// no body at all and stop at their `value`.
///
/// Written as three independent lookups rather than a chain of `?`: a
/// `const_item` has no `declaration` field, so an early return there would
/// skip the `value` lookup and leave the initializer in the signature.
fn signature_cut(def: Node) -> Option<Node> {
    if let Some(body) = def.child_by_field_name("body") {
        return Some(body);
    }
    if let Some(declaration) = def.child_by_field_name("declaration") {
        if let Some(body) = declaration.child_by_field_name("body") {
            return Some(body);
        }
    }
    if let Some(value) = def.child_by_field_name("value") {
        return Some(value);
    }
    // Assignments cut at the value ONLY when a type annotation carries the
    // contract. Rust cuts a `const`'s value because the type always survives;
    // Python's annotation is optional, so cutting an unannotated assignment
    // would leave a bare identifier with no kind, no type, and no value. The
    // Rust cycle made the same distinction for `type` aliases, which it
    // refused to cut because "an alias without its target would be useless".
    if def.kind() == "assignment" && def.child_by_field_name("type").is_some() {
        return def.child_by_field_name("right");
    }
    None
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
    cut: Option<Node>,
) -> String {
    let start = (spec.sig_start)(def).start_byte();
    let end = cut
        .map(|b| b.start_byte())
        .unwrap_or_else(|| def.end_byte())
        .max(start)
        .min(text.len());
    let raw = match (owner, name) {
        (Some(owner), Some(name)) if name.start_byte() >= start && name.start_byte() <= end => {
            format!(
                "{}{}{}{}",
                text.get(start..name.start_byte()).unwrap_or(""),
                owner,
                spec.owner_sep,
                text.get(name.start_byte()..end).unwrap_or("")
            )
        }
        _ => text.get(start..end).unwrap_or("").to_string(),
    };
    // A backslash immediately followed by a newline is Python's explicit line
    // continuation: it is not whitespace, so it survives collapsing and
    // strands the trailing-strip loop one character short of the `=` it must
    // remove (`Z: int = \` never reduces to `Z: int`). Gated per-language:
    // JS and TS allow the identical sequence inside a string literal, and a
    // default-parameter value sits inside the retained signature span, so
    // joining it there would silently rewrite `"x\<newline>y"` to `"x y"`.
    let raw = if spec.join_continuations {
        // CRLF line endings make the continuation sequence `\` `\r` `\n`, not
        // `\` `\n` — replace the CRLF form first, or a CRLF file leaves the
        // backslash unjoined (`Z: int = \` survives untouched), exactly the
        // defect this join exists to prevent.
        raw.replace("\\\r\n", " ").replace("\\\n", " ")
    } else {
        raw
    };
    let mut sig = tidy_punctuation(collapse_whitespace(&raw));
    loop {
        match sig.chars().last() {
            Some(c) if spec.strip_trailing.contains(&c) => {
                sig.pop();
                sig = sig.trim_end().to_string();
            }
            _ => break,
        }
    }
    if def.kind() == "assignment" && def.child_by_field_name("type").is_none() {
        sig = truncate_value(sig);
    }
    sig
}

/// Characters of an unannotated assignment's value kept in a signature.
/// Measured: the smallest round budget under which every real constant in the
/// audit corpora survives intact, truncating only a multi-line prompt and a
/// multi-line user-agent string.
const PY_VALUE_BUDGET: usize = 48;

/// Bound a retained assignment value. Counts `chars()`, never bytes: the audit
/// corpus contains a Cyrillic prompt literal that a byte slice would split
/// mid-scalar and panic on.
fn truncate_value(sig: String) -> String {
    let Some(eq) = sig.find(" = ") else {
        return sig;
    };
    let split = eq + " = ".len();
    let (head, value) = sig.split_at(split);
    if value.chars().count() <= PY_VALUE_BUDGET {
        return sig;
    }
    let kept: String = value.chars().take(PY_VALUE_BUDGET).collect();
    format!("{head}{kept}…")
}

/// Remove the artifacts whitespace collapsing leaves around the punctuation of
/// a multi-line parameter list: `fn f( a: u8, b: u8, )` becomes
/// `fn f(a: u8, b: u8)`. The trailing comma before a collapsed closing
/// delimiter always arrives as `, )` (with a space, from the newline that
/// separated them), so matching that sequence is enough to remove it; a
/// genuine one-element tuple written `(1,)` has no such space and is left
/// alone.
///
/// Known limitation: a one-element tuple that was itself wrapped across lines
/// collapses to `( u8, )` and is reduced to `(u8)`, losing its arity. That text
/// is identical to a wrapped one-parameter list, which must reduce to `(u8)`,
/// so no rule over the collapsed string can tell them apart — separating them
/// would take the parse tree. Signatures are documentation, not compiled code,
/// and the shape is rare enough to accept.
///
/// Unlike `placement`, `vis_filter` and `owner_sep`, this pass is
/// not a `LangSpec` field: it runs inside `build_signature` for all five
/// languages, not just Rust. It is a plain substring substitution over the
/// collapsed text, so it also rewrites punctuation sitting inside a string
/// literal that survives into a signature — a measured example is a Python
/// default argument `gamma=" )"`, which renders as `gamma=")"`. Rust is immune
/// because `signature_cut` stops a `const`/`static` signature at `value:`, but
/// Python and JS/TS default arguments sit inside the retained span.
fn tidy_punctuation(mut sig: String) -> String {
    for (from, to) in [("( ", "("), (", )", ")"), (" )", ")"), (" ,", ",")] {
        while sig.contains(from) {
            sig = sig.replace(from, to);
        }
    }
    sig
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.trim().chars() {
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
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
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
        return doc.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
    }
    None
}

fn derive_name_from_path(rel_path: &str) -> String {
    let base = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let stem = base.split('.').next().unwrap_or(base);
    stem.replace(['_', '-'], " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn wrapped_parameter_lists_are_tidied_in_every_language() {
        // `tidy_punctuation` is not gated by `LangSpec` the way owner/vis
        // machinery is — it runs inside `build_signature` for all five
        // languages. Pin that as intended behavior, not something left to be
        // discovered: this also exercises the documented caveat that a
        // string literal's punctuation (`gamma=" )"`) is rewritten too,
        // since Python's signature span isn't cut before its default
        // arguments the way Rust's `const`/`static` values are.
        let py = "def wrapped(\n    alpha,\n    beta=(1,),\n    gamma=\" )\",\n):\n    pass\n";
        let e = CodeExtractor.extract("t.py", py);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "def wrapped(alpha, beta=(1,), gamma=\")\")"),
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
        for ext in ["rs", "py", "js", "ts", "go"] {
            let spec = lang_for_ext(ext).unwrap_or_else(|| panic!("no spec for {ext}"));
            if let Err(e) = Query::new(&spec.language, spec.query_src) {
                panic!("query for {ext} failed to compile: {e:?}");
            }
        }
    }
}
