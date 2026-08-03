use crate::formats::{summarize, Extractor};
use crate::model::{slugify, Entity, SourceKind};
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

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
}

pub(crate) fn keep_all(_name: &str) -> bool {
    true
}

pub(crate) fn keep_any_vis(_vis: &str) -> bool {
    true
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
    /// A top-level declaration: no owner, and a `@name` capture.
    Free,
    /// A trait-impl header. The only pattern that captures a `@def` without a
    /// `@name` is `(impl_item trait: (_) type: (_)) @def`.
    Header,
    /// A method or associated item: both an owner and a `@name`.
    Member,
}

fn extract_code(ext: &str, text: &str) -> Option<CodeInfo> {
    let spec = lang_for_ext(ext)?;
    let mut parser = Parser::new();
    parser.set_language(&spec.language).ok()?;
    let tree = parser.parse(text, None)?;
    let query = Query::new(&spec.language, spec.query_src).ok()?;
    let def_idx = query.capture_index_for_name("def");
    let name_idx = query.capture_index_for_name("name");
    let vis_idx = query.capture_index_for_name("vis");
    let imp_idx = query.capture_index_for_name("import");

    // (signature, kind). See `ItemKind` for why the kind is recorded here
    // rather than recovered from the signature string later.
    let mut collected: Vec<(String, ItemKind)> = Vec::new();
    let mut imports = Vec::new();
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    for m in matches {
        let mut def_node: Option<Node> = None;
        let mut name_node: Option<Node> = None;
        let mut vis_text: Option<&str> = None;
        for cap in m.captures {
            if Some(cap.index) == def_idx {
                def_node = Some(cap.node);
            } else if Some(cap.index) == name_idx {
                name_node = Some(cap.node);
            } else if Some(cap.index) == vis_idx {
                vis_text = text.get(cap.node.byte_range());
            } else if Some(cap.index) == imp_idx {
                if let Some(raw) = text.get(cap.node.byte_range()) {
                    imports.push(raw.trim().trim_matches(['"', '\'']).to_string());
                }
            }
        }
        if let Some(def) = def_node {
            let Placement::Scoped(chain) = (spec.placement)(def, text) else {
                continue;
            };
            let name_text = name_node.and_then(|n| text.get(n.byte_range()));
            let keep = name_text.map(|n| (spec.name_filter)(n)).unwrap_or(true)
                && vis_text.map(|v| (spec.vis_filter)(v)).unwrap_or(true);
            if keep {
                let owner = if chain.is_empty() {
                    None
                } else {
                    Some(chain.join(spec.owner_sep))
                };
                let sig = build_signature(
                    text,
                    def,
                    name_node,
                    owner.as_deref(),
                    spec.owner_sep,
                    signature_cut(def),
                    spec.strip_trailing,
                );
                if !sig.is_empty() {
                    let kind = if name_node.is_none() {
                        ItemKind::Header
                    } else if owner.is_some() {
                        ItemKind::Member
                    } else {
                        ItemKind::Free
                    };
                    collected.push((sig, kind));
                }
            }
        }
    }
    // Sorting a `(String, ItemKind)` tuple ties on `ItemKind`'s derived order
    // only when two entries share a signature. Deduplication below is keyed
    // on the signature alone, so which kind wins such a tie is irrelevant:
    // the surviving text is identical either way, and `symbols` (built from
    // the signature only, after dedup) cannot observe the difference.
    collected.sort();
    // Deduplicate on the signature alone: comparing the full tuple would let
    // a future divergence between a signature and its recorded kind smuggle
    // a duplicate through instead of surfacing the mismatch.
    collected.dedup_by(|a, b| a.0 == b.0);
    imports.sort();
    imports.dedup();

    // The summary fallback must not be a qualified method. `symbols` is
    // sorted, so on every extractor module `fn <X as Extractor>::extensions`
    // would outrank `pub struct X` and become the page's one-line summary.
    // Prefer a free item, then an `impl` header (covering generic and
    // `unsafe` impls, which no longer have a recognizable signature prefix
    // to sniff), then whatever sorts first.
    let summary_fallback = collected
        .iter()
        .find(|(_, kind)| *kind == ItemKind::Free)
        .or_else(|| collected.iter().find(|(_, kind)| *kind == ItemKind::Header))
        .or_else(|| collected.first())
        .map(|(sig, _)| sig.clone());

    let symbols: Vec<String> = collected.into_iter().map(|(sig, _)| sig).collect();

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
    def.child_by_field_name("value")
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
    owner_sep: &str,
    cut: Option<Node>,
    strip_trailing: &[char],
) -> String {
    let start = def.start_byte();
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
                owner_sep,
                text.get(name.start_byte()..end).unwrap_or("")
            )
        }
        _ => text.get(start..end).unwrap_or("").to_string(),
    };
    let mut sig = tidy_punctuation(collapse_whitespace(&raw));
    loop {
        match sig.chars().last() {
            Some(c) if strip_trailing.contains(&c) => {
                sig.pop();
                sig = sig.trim_end().to_string();
            }
            _ => break,
        }
    }
    sig
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
    fn rust_qualification_does_not_leak_into_other_languages() {
        // Python nested defs and class methods are captured today and must
        // stay captured, unqualified: the module-level guard and the owner
        // walk are Rust-only.
        let py = "class Registry:\n    def register(self, x):\n        pass\n\ndef outer():\n    def inner():\n        pass\n    return inner\n";
        let e = CodeExtractor.extract("t.py", py);
        for expected in ["def register(self, x)", "def inner()", "def outer()"] {
            assert!(
                e.symbols.iter().any(|s| s == expected),
                "{expected} missing: {:?}",
                e.symbols
            );
        }
        assert!(
            !e.symbols.iter().any(|s| s.contains("::")),
            "Python symbols must not be qualified: {:?}",
            e.symbols
        );

        let go = "package main\n\nfunc Foo(a int) string {\n\treturn \"\"\n}\n";
        let e = CodeExtractor.extract("m.go", go);
        assert!(
            e.symbols.iter().any(|s| s == "func Foo(a int) string"),
            "symbols: {:?}",
            e.symbols
        );
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
    }
}
