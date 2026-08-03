use crate::formats::{summarize, Extractor};
use crate::model::{slugify, Entity, SourceKind};
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

pub struct CodeExtractor;

struct LangSpec {
    lang_name: &'static str,
    language: Language,
    /// Query capturing:
    ///   @def    - the definition node whose text is shortened to a signature
    ///   @name   - the definition's name (used for post-hoc export/visibility filtering)
    ///   @import - a module path node
    ///   @vis    - the visibility-modifier text that `vis_filter` gates on
    query_src: &'static str,
    /// Keep a captured definition only if its @name text passes this filter.
    /// Used where the grammar can't express the gate structurally (Python's
    /// leading-underscore convention, Go's capitalized-identifier convention).
    name_filter: fn(&str) -> bool,
    /// Applied to the text of a `@vis` capture. A pattern that captures no
    /// `@vis` is not gated at all — items inside a trait impl or a trait
    /// declaration carry no visibility modifier yet are public through the
    /// trait.
    vis_filter: fn(&str) -> bool,
    /// Trailing characters to strip off a built signature (e.g. Rust's `;`
    /// on a body-less unit/tuple struct, Python's `:` after the parameter list).
    strip_trailing: &'static [char],
    /// Resolves the owner of a definition nested inside a type or trait scope,
    /// already rendered for splicing into the signature. `None` means the item
    /// is free-standing and its signature is used verbatim.
    owner_of: fn(Node, &str) -> Option<String>,
    /// Separator between owner and member name in a qualified signature.
    owner_sep: &'static str,
    /// Rejects a captured definition outright, for scopes the query cannot
    /// express. Per-language rather than shared: Python's pattern captures
    /// nested `def`s inside function bodies today, and a shared module-level
    /// rule would silently change its output.
    def_filter: fn(Node) -> bool,
}

fn keep_all(_name: &str) -> bool {
    true
}

fn keep_python_public(name: &str) -> bool {
    !name.starts_with('_')
}

fn keep_go_exported(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

fn keep_any_vis(_vis: &str) -> bool {
    true
}

/// Rust only. `(visibility_modifier)` also covers `pub(crate)`, `pub(super)`,
/// and `pub(in path)`, none of which leave the crate; bare `pub` is the only
/// one that belongs in `## Exports`.
fn keep_bare_pub(vis: &str) -> bool {
    vis == "pub"
}

fn no_owner(_def: Node, _text: &str) -> Option<String> {
    None
}

fn any_def(_def: Node) -> bool {
    true
}

/// The owner of a Rust definition, rendered as it will appear in the
/// signature:
///
///   * inherent `impl` — the type name with generic arguments stripped, so
///     `impl<T> Holder<T>` yields `Holder::get`, valid Rust that sorts beside
///     the type's other methods;
///   * trait `impl` — `<Type as Trait>`, Rust's own disambiguation syntax. The
///     trait *must* be part of the owner: `impl Display for Foo` and
///     `impl Debug for Foo` both define `fn fmt(&self, …) -> Result`, which
///     collapse to a single line under `dedup` if only the type is named. It
///     also keeps the full type text, so `impl Encode for Vec<u8>` and
///     `… for Vec<u16>` stay distinct, and it renders exotic targets as valid
///     paths (`<&Foo as Trait>::m`).
///   * `trait` declaration — the trait's own name.
fn rust_owner(def: Node, text: &str) -> Option<String> {
    let parent = def.parent()?;
    if parent.kind() != "declaration_list" {
        return None;
    }
    let holder = parent.parent()?;
    match holder.kind() {
        "impl_item" => {
            let ty = text.get(holder.child_by_field_name("type")?.byte_range())?;
            match holder.child_by_field_name("trait") {
                Some(tr) => Some(format!("<{} as {}>", ty, text.get(tr.byte_range())?)),
                None => Some(ty.split('<').next().unwrap_or(ty).trim().to_string()),
            }
        }
        "trait_item" => text
            .get(holder.child_by_field_name("name")?.byte_range())
            .map(str::to_string),
        _ => None,
    }
}

/// True when `node` is reachable from the file root through module and
/// type-definition scopes only. Reaching a `block` means the item lives inside
/// a function body — a fixture `impl` written in a helper is not part of the
/// module's public surface, and tree-sitter queries match at any depth.
fn rust_module_level(mut node: Node) -> bool {
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "source_file" | "mod_item" | "declaration_list" | "impl_item" | "trait_item" => {
                node = parent
            }
            _ => return false,
        }
    }
    true
}

fn lang_for_ext(ext: &str) -> Option<LangSpec> {
    Some(match ext {
        "rs" => LangSpec {
            lang_name: "rust",
            language: tree_sitter_rust::LANGUAGE.into(),
            // Requiring `(visibility_modifier)` excludes private items
            // structurally; `vis_filter` then drops the restricted forms
            // (`pub(crate)` and friends) that the node kind also covers.
            query_src: r#"
                (function_item (visibility_modifier) @vis name: (identifier) @name) @def
                (struct_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (enum_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (trait_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (const_item (visibility_modifier) @vis name: (identifier) @name) @def
                (static_item (visibility_modifier) @vis name: (identifier) @name) @def
                (type_item (visibility_modifier) @vis name: (type_identifier) @name) @def
                (use_declaration argument: (_) @import)
            "#,
            name_filter: keep_all,
            vis_filter: keep_bare_pub,
            strip_trailing: &[';', '='],
            owner_of: rust_owner,
            owner_sep: "::",
            def_filter: rust_module_level,
        },
        "py" => LangSpec {
            lang_name: "python",
            language: tree_sitter_python::LANGUAGE.into(),
            // The grammar has no notion of "public"; capture every def/class
            // and drop names starting with `_` in name_filter (PEP 8 convention).
            query_src: r#"
                (function_definition name: (identifier) @name) @def
                (class_definition name: (identifier) @name) @def
                (import_statement name: (dotted_name) @import)
                (import_from_statement module_name: (dotted_name) @import)
            "#,
            name_filter: keep_python_public,
            vis_filter: keep_any_vis,
            strip_trailing: &[':'],
            owner_of: no_owner,
            owner_sep: "",
            def_filter: any_def,
        },
        "js" => LangSpec {
            lang_name: "javascript",
            language: tree_sitter_javascript::LANGUAGE.into(),
            // Only symbols wrapped in an `export_statement` are captured at
            // all, so a bare `function helper() {}` never matches.
            query_src: r#"
                (export_statement declaration: (function_declaration name: (identifier) @name)) @def
                (export_statement declaration: (class_declaration name: (identifier) @name)) @def
                (import_statement source: (string) @import)
            "#,
            name_filter: keep_all,
            vis_filter: keep_any_vis,
            strip_trailing: &[],
            owner_of: no_owner,
            owner_sep: "",
            def_filter: any_def,
        },
        "ts" => LangSpec {
            lang_name: "typescript",
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            query_src: r#"
                (export_statement declaration: (function_declaration name: (identifier) @name)) @def
                (export_statement declaration: (class_declaration name: (type_identifier) @name)) @def
                (import_statement source: (string) @import)
            "#,
            name_filter: keep_all,
            vis_filter: keep_any_vis,
            strip_trailing: &[],
            owner_of: no_owner,
            owner_sep: "",
            def_filter: any_def,
        },
        "go" => LangSpec {
            lang_name: "go",
            language: tree_sitter_go::LANGUAGE.into(),
            // Go has no `export` keyword; "exported" means the identifier's
            // first rune is uppercase. The query can't express that, so it
            // captures every func/type and name_filter drops the unexported ones.
            query_src: r#"
                (function_declaration name: (identifier) @name) @def
                (type_declaration (type_spec name: (type_identifier) @name)) @def
                (import_spec path: (interpreted_string_literal) @import)
            "#,
            name_filter: keep_go_exported,
            vis_filter: keep_any_vis,
            strip_trailing: &[],
            owner_of: no_owner,
            owner_sep: "",
            def_filter: any_def,
        },
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
            strip_rust_test_modules(text).unwrap_or_else(|| text.to_string())
        } else {
            text.to_string()
        };

        let (lang_name, symbols, imports, docstring) = match extract_code(ext, &source) {
            Some(v) => v,
            None => (ext.to_string(), Vec::new(), Vec::new(), None),
        };

        let docstring = docstring.or_else(|| leading_doc(&source, ext));
        let first_sig = symbols.first().map(String::as_str);
        // `body` is deliberately NOT scanned for a summary: source code is not
        // prose, so letting summarize() hunt line-by-line for a "real
        // sentence" produces garbage. Only a real docstring, or (failing
        // that) an exported signature, is an acceptable summary.
        let summary = summarize(None, docstring.as_deref(), "", first_sig);
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

type CodeInfo = (String, Vec<String>, Vec<String>, Option<String>);

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

    let mut symbols = Vec::new();
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
            let name_text = name_node.and_then(|n| text.get(n.byte_range()));
            let keep = name_text.map(|n| (spec.name_filter)(n)).unwrap_or(true)
                && vis_text.map(|v| (spec.vis_filter)(v)).unwrap_or(true)
                && (spec.def_filter)(def);
            if keep {
                let owner = (spec.owner_of)(def, text);
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
                    symbols.push(sig);
                }
            }
        }
    }
    symbols.sort();
    symbols.dedup();
    imports.sort();
    imports.dedup();

    let docstring = if ext == "py" {
        python_docstring(&tree, text)
    } else {
        None
    };

    Some((spec.lang_name.to_string(), symbols, imports, docstring))
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

/// A Python module-level docstring: the first non-comment top-level
/// statement, if it is a bare string expression, with its first non-empty
/// inner line returned. Handles both single-line (`"""Doc."""`) and
/// multi-line (`"""\nDoc.\n"""`) docstrings, since it reads the AST's
/// `string_content` node rather than assuming the doc text is on line 1.
fn python_docstring(tree: &Tree, text: &str) -> Option<String> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let first_stmt = root.children(&mut cursor).find(|n| n.kind() != "comment")?;
    if first_stmt.kind() != "expression_statement" {
        return None;
    }
    let mut inner_cursor = first_stmt.walk();
    let string_node = first_stmt
        .children(&mut inner_cursor)
        .find(|n| n.kind() == "string")?;
    let mut content_cursor = string_node.walk();
    let content_node = string_node
        .children(&mut content_cursor)
        .find(|n| n.kind() == "string_content")?;
    let raw = text.get(content_node.byte_range())?;
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
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

/// Remove `#[cfg(test)]`-annotated `mod` items from Rust source, replacing
/// each with a one-line omission marker (`// [tests omitted: mod <name>,
/// <N> lines]`). The spliced span starts at the first attribute in the
/// contiguous run of attributes directly above the `mod` (so `#[cfg(test)]`
/// itself is removed) and ends at the module's closing brace; `<N>` is that
/// span's line count. Returns `None` when there is nothing to strip or the
/// source fails to parse — the caller keeps the raw text.
fn strip_rust_test_modules(text: &str) -> Option<String> {
    let language: Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(text, None)?;
    let query = Query::new(&language, "(mod_item) @m").ok()?;
    let mut cursor = QueryCursor::new();

    // (start, end, mod name) per cfg(test) module, at any nesting depth.
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    let matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    for m in matches {
        for cap in m.captures {
            let node = cap.node;
            // Walk the contiguous run of attribute_items directly above the
            // mod; the whole run is spliced when any of them is cfg(test).
            let mut start = node.start_byte();
            let mut is_test_mod = false;
            let mut prev = node.prev_named_sibling();
            while let Some(p) = prev {
                if p.kind() != "attribute_item" {
                    break;
                }
                let attr: String = text
                    .get(p.byte_range())
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                if attr == "#[cfg(test)]" {
                    is_test_mod = true;
                }
                start = p.start_byte();
                prev = p.prev_named_sibling();
            }
            if !is_test_mod {
                continue;
            }
            let name = node
                .child_by_field_name("name")
                .and_then(|n| text.get(n.byte_range()))
                .unwrap_or("?")
                .to_string();
            spans.push((start, node.end_byte(), name));
        }
    }
    if spans.is_empty() {
        return None;
    }
    spans.sort_by_key(|s| s.0);

    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for (start, end, name) in spans {
        if start < pos {
            continue; // nested inside a module already removed
        }
        let lines = text[start..end].lines().count();
        out.push_str(&text[pos..start]);
        out.push_str(&format!("// [tests omitted: mod {name}, {lines} lines]"));
        // The marker is a line comment. Anything that followed the module's
        // closing brace on the same line would otherwise be commented out —
        // cosmetic in `## Body`, but a silent loss of exports once symbols
        // are extracted from this text.
        let rest = &text[end..];
        if !rest.is_empty() && !rest.starts_with('\n') {
            out.push('\n');
        }
        pos = end;
    }
    out.push_str(&text[pos..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_signatures_gated_and_imports() {
        let src = "//! Module docs.\nuse crate::graph::build;\npub fn render_page(x: i32) -> String { String::new() }\nfn private_helper() {}\n";
        let e = CodeExtractor.extract("src/rewrite.rs", src);
        match &e.kind {
            crate::model::SourceKind::Code { lang } => assert_eq!(lang, "rust"),
            _ => panic!("expected Code kind"),
        }
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub fn render_page(x: i32) -> String"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("private_helper")));
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert_eq!(e.summary.as_deref(), Some("Module docs."));
    }

    #[test]
    fn rust_restricted_visibility_is_not_an_export() {
        let src = "pub fn public_one() {}\npub(crate) fn crate_only() {}\npub(super) fn super_only() {}\npub(crate) struct CrateType;\nstruct Holder;\nimpl Holder {\n    pub fn kept(&self) {}\n    pub(crate) fn impl_crate_only(&self) {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols.iter().any(|s| s == "pub fn public_one()"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s.contains("kept")),
            "a bare-pub inherent method must survive: {:?}",
            e.symbols
        );
        for leaked in ["crate_only", "super_only", "CrateType", "impl_crate_only"] {
            assert!(
                !e.symbols.iter().any(|s| s.contains(leaked)),
                "{leaked} is not exported outside the crate: {:?}",
                e.symbols
            );
        }
    }

    #[test]
    fn rust_trait_visibility_gated() {
        let src = "pub trait Public {\n    fn m(&self);\n}\ntrait Private {\n    fn n(&self);\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols.iter().any(|s| s == "pub trait Public"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("Private")),
            "private trait leaked as export: {:?}",
            e.symbols
        );
    }

    #[test]
    fn rust_module_level_const_static_and_type_alias() {
        let src = "pub const CACHE_VERSION: u32 = 1;\npub static NAME: &str = \"x\";\npub type Pack = Vec<u8>;\nconst PRIVATE: u8 = 3;\n";
        let e = CodeExtractor.extract("t.rs", src);
        // A const's contract is its type; the literal stays visible in `## Body`,
        // exactly as a function's body does.
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub const CACHE_VERSION: u32"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "pub static NAME: &str"),
            "symbols: {:?}",
            e.symbols
        );
        // An alias without its target would be useless, and `type_item` has no
        // `value:` field, so nothing is cut.
        assert!(
            e.symbols.iter().any(|s| s == "pub type Pack = Vec<u8>"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            !e.symbols.iter().any(|s| s.contains("PRIVATE")),
            "symbols: {:?}",
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
    fn inherent_impl_methods_are_qualified_with_their_type() {
        let src = "pub struct Wiki;\nimpl Wiki {\n    pub fn search(&self, q: &str) -> Vec<Hit> { todo!() }\n    fn helper(&self) {}\n}\npub fn free_function() {}\n";
        let e = CodeExtractor.extract("query.rs", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub fn Wiki::search(&self, q: &str) -> Vec<Hit>"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "pub fn free_function()"),
            "a top-level function must stay unqualified: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("helper")));
    }

    #[test]
    fn generic_inherent_impl_strips_type_arguments() {
        let src = "impl<T: Clone> Holder<T> {\n    pub fn get(&self) -> T { todo!() }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        // `Holder::get` is valid Rust and sorts beside the type's other
        // methods; `Holder<T>::get` would be neither.
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "pub fn Holder::get(&self) -> T"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn function_local_items_are_not_module_exports() {
        let src = "pub fn outer() {\n    pub struct Local;\n    impl Local {\n        pub fn hidden(&self) {}\n    }\n    pub fn nested() {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols.iter().any(|s| s == "pub fn outer()"),
            "symbols: {:?}",
            e.symbols
        );
        for leaked in ["Local", "hidden", "nested"] {
            assert!(
                !e.symbols.iter().any(|s| s.contains(leaked)),
                "{leaked} lives in a function body and is not module surface: {:?}",
                e.symbols
            );
        }
    }

    #[test]
    fn items_in_an_inline_module_stay_unqualified() {
        let src = "pub mod inner {\n    pub fn in_mod() {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            e.symbols.iter().any(|s| s == "pub fn in_mod()"),
            "symbols: {:?}",
            e.symbols
        );
    }

    #[test]
    fn python_signatures_gated_docstring_and_imports() {
        let src = "\"\"\"\nTop docstring.\n\"\"\"\nimport os\nfrom graph import build\ndef extract_all(d):\n    pass\ndef _private():\n    pass\n";
        let e = CodeExtractor.extract("extractor.py", src);
        assert!(
            e.symbols.iter().any(|s| s == "def extract_all(d)"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("_private")));
        assert!(e.imports.iter().any(|i| i.contains("graph")));
        assert!(e.imports.iter().any(|i| i.contains("os")));
        assert_eq!(e.summary.as_deref(), Some("Top docstring."));
    }

    #[test]
    fn js_export_gated_signatures() {
        let src = "export function foo(a, b) {\n  return a + b;\n}\nfunction helper() {\n  return 1;\n}\n";
        let e = CodeExtractor.extract("mod.js", src);
        assert!(
            e.symbols.iter().any(|s| s == "export function foo(a, b)"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("helper")));
    }

    #[test]
    fn js_import_source_captured() {
        let src = "import { thing } from \"./thing.js\";\nexport function foo() {}\n";
        let e = CodeExtractor.extract("mod.js", src);
        assert!(
            e.imports.iter().any(|i| i.contains("thing.js")),
            "imports: {:?}",
            e.imports
        );
    }

    #[test]
    fn ts_export_gated_signatures() {
        let src = "export class Bar {}\nexport function baz(x: number): void {\n  return;\n}\nfunction hidden() {\n  return 1;\n}\n";
        let e = CodeExtractor.extract("mod.ts", src);
        assert!(
            e.symbols
                .iter()
                .any(|s| s == "export function baz(x: number): void"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(
            e.symbols.iter().any(|s| s == "export class Bar"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("hidden")));
    }

    #[test]
    fn go_exported_gated_signatures_and_imports() {
        let src = "package main\n\nimport \"fmt\"\n\nfunc Foo(a int) string {\n\treturn fmt.Sprint(a)\n}\n\nfunc bar() {\n}\n";
        let e = CodeExtractor.extract("main.go", src);
        assert!(
            e.symbols.iter().any(|s| s == "func Foo(a int) string"),
            "symbols: {:?}",
            e.symbols
        );
        assert!(!e.symbols.iter().any(|s| s.contains("bar")));
        assert!(e.imports.iter().any(|i| i.contains("fmt")));
    }

    #[test]
    fn rust_test_module_stripped_with_marker() {
        let src = "//! Docs.\npub fn real() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn checks_real() {\n        real();\n    }\n}\n";
        let e = CodeExtractor.extract("src/thing.rs", src);
        assert!(
            !e.body.contains("checks_real"),
            "test fn text must be gone: {}",
            e.body
        );
        assert!(!e.body.contains("#[cfg(test)]"), "body: {}", e.body);
        // Removed span = the attribute line through the closing brace: 9 lines.
        assert!(
            e.body.contains("// [tests omitted: mod tests, 9 lines]"),
            "body: {}",
            e.body
        );
        assert!(e.body.contains("pub fn real()"), "body: {}", e.body);
    }

    #[test]
    fn rust_without_test_module_round_trips_byte_identical() {
        let src = "//! Docs.\nuse std::fmt;\npub fn only() {}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_eq!(e.body, src);
    }

    #[test]
    fn rust_two_test_modules_get_two_markers_in_source_order() {
        let src = "pub fn a() {}\n\n#[cfg(test)]\nmod early_tests {\n    #[test]\n    fn t1() {}\n}\n\npub fn b() {}\n\n#[cfg(test)]\nmod late_tests {\n    #[test]\n    fn t2() {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        let early = e.body.find("// [tests omitted: mod early_tests, 5 lines]");
        let late = e.body.find("// [tests omitted: mod late_tests, 5 lines]");
        assert!(early.is_some() && late.is_some(), "body: {}", e.body);
        assert!(
            early.unwrap() < late.unwrap(),
            "markers out of order: {}",
            e.body
        );
        assert!(!e.body.contains("fn t1") && !e.body.contains("fn t2"));
    }

    #[test]
    fn rust_entirely_test_module_body_is_just_the_marker() {
        let src = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert_eq!(e.body.trim(), "// [tests omitted: mod tests, 5 lines]");
    }

    #[test]
    fn code_after_a_test_module_on_the_same_line_is_not_commented_out() {
        let src = "pub fn before() {}\n#[cfg(test)]\nmod tests { #[test] fn t() {} } pub fn after() -> u8 { 1 }\n";
        let e = CodeExtractor.extract("t.rs", src);
        let marker_line = e
            .body
            .lines()
            .find(|l| l.contains("[tests omitted"))
            .unwrap_or_else(|| panic!("no marker in body: {}", e.body));
        assert!(
            !marker_line.contains("pub fn after"),
            "code following the module was swallowed by the marker comment: {marker_line}"
        );
    }

    #[test]
    fn cfg_test_matches_modulo_whitespace_but_other_cfgs_do_not() {
        // Interior whitespace in the attribute still counts as cfg(test).
        let ws = "#[cfg( test )]\nmod tests {}\npub fn x() {}\n";
        let e = CodeExtractor.extract("a.rs", ws);
        assert!(
            e.body.contains("// [tests omitted: mod tests, 2 lines]"),
            "body: {}",
            e.body
        );

        // A different cfg is NOT a test module.
        let feature = "#[cfg(feature = \"extra\")]\nmod extra {}\npub fn x() {}\n";
        let e2 = CodeExtractor.extract("b.rs", feature);
        assert_eq!(e2.body, feature);
    }

    #[test]
    fn non_rust_bodies_are_untouched() {
        let src = "def test_visible():\n    pass\n";
        let e = CodeExtractor.extract("t.py", src);
        assert_eq!(e.body, src);
    }

    #[test]
    fn symbols_and_imports_come_from_the_test_stripped_source() {
        let src = "pub struct Fixture;\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    use tempfile::tempdir;\n\n    impl Default for Fixture {\n        fn default() -> Self { Fixture }\n    }\n\n    #[test]\n    fn t() {\n        let _ = tempdir();\n    }\n}\n";
        let e = CodeExtractor.extract("t.rs", src);
        assert!(
            !e.imports.iter().any(|i| i == "super::*"),
            "test-only import leaked: {:?}",
            e.imports
        );
        assert!(
            !e.imports.iter().any(|i| i.contains("tempfile")),
            "test-only import leaked: {:?}",
            e.imports
        );
        assert!(
            e.symbols.iter().any(|s| s == "pub struct Fixture"),
            "real export lost: {:?}",
            e.symbols
        );
    }
}
