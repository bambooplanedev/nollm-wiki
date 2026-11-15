//! Signature rendering: comment-free, whitespace-collapsed text of a definition head with its retained value, spliced with its owner.

use super::{LangSpec, Shape};
use tree_sitter::Node;

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
pub(crate) fn is_comment_kind(kind: &str) -> bool {
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
/// Three segment kinds, decided structurally rather than by searching the text:
///
///   * a **comment** is dropped. A `///` on a field inside an enum variant's
///     body would otherwise collapse into the signature
///     (`MatchErrorKind::UnsupportedStream { /// The match semantics … got: MatchKind, }`),
///     and a `//` line comment inside a one-line signature would comment out
///     everything after it for anyone who copied it. This also covers a
///     comment between a decorator and its definition, and one inside a
///     retained value.
///   * a **string literal** is collapsed but never tidied, so a `" , "` stays
///     `" , "` instead of being rewritten to `", "`.
///   * everything else is collapsed and tidied.
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
pub(super) fn shared_declaration_signature(
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
pub(super) fn build_signature(
    text: &str,
    def: Node,
    name: Option<Node>,
    owner: Option<&str>,
    spec: &LangSpec,
    shape: &Shape,
) -> String {
    if let Some(sig) = name
        .and_then(|n| text.get(n.byte_range()))
        .and_then(|n| (spec.fixed_signature)(def, n))
    {
        return sig;
    }
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
    let mut sig = raw
        .trim()
        .trim_end_matches(|c: char| c.is_whitespace() || spec.strip_trailing.contains(&c))
        .to_string();
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
/// It is a plain substring substitution, applied only outside string literals:
/// `render_span` withholds those spans, so a Python default `gamma=" )"` and a
/// Rust `pub const SEP: &str = " , "` keep their own punctuation. Pinned by
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
