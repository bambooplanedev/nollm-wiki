//! Doc extraction: plain-comment doc blocks, one-line item docs and the leading doc of a file.

use super::is_cpp;
use crate::support::{clip, first_real_sentence, ITEM_DOC_TEXT_MAX_CHARS};

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
            .flat_map(crate::support::name_words)
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

/// An item's doc is one line in `items.json`, so it is cut to this many
/// chars, the trailing `…` included.
const ITEM_DOC_MAX_CHARS: usize = 120;

/// The first sentence of an item's raw doc text, capped at
/// `ITEM_DOC_MAX_CHARS`. Counts `chars()`, never bytes, so a multi-byte doc
/// is never split mid-scalar.
pub(super) fn item_doc_line(raw: &str) -> Option<String> {
    first_real_sentence(raw).map(|line| cap_chars(&line, ITEM_DOC_MAX_CHARS))
}

/// `s` unchanged, or its first `max - 1` chars and a `…` when it has more than
/// `max`.
fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max - 1).chain(['…']).collect()
}

/// An item's whole raw doc on one line, capped at `ITEM_DOC_TEXT_MAX_CHARS`
/// (the `…` included); `None` when it is just `doc`.
pub(super) fn item_doc_text(raw: &str, doc: Option<&str>) -> Option<String> {
    let text = clip(raw, ITEM_DOC_TEXT_MAX_CHARS);
    (!text.is_empty() && doc != Some(text.as_str())).then_some(text)
}

/// Module-level doc comment, one paragraph, if present. The fallback for
/// languages without an AST-verified docstring convention (everything but
/// Python).
///
/// Empty doc lines are SKIPPED rather than terminating the scan: a Doxygen
/// block opens with a bare `/**`, which strips to nothing, and aborting there
/// would leave the commonest C++ comment shape with no docstring at all.
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
pub(super) fn leading_doc(text: &str, ext: &str) -> Option<String> {
    let cpp = is_cpp(ext);
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
            // A shebang or a `# shellcheck` directive is no doc, but may open
            // the file: skipped as a gap.
            "sh" | "bash" if line.starts_with("#!") || line.starts_with("# shellcheck ") => {
                Some("")
            }
            "sh" | "bash" => line.strip_prefix('#'),
            "js" | "mjs" | "cjs" | "ts" | "jsx" | "tsx" | "vue" | "svelte" | "go" => {
                line.strip_prefix("//").or_else(|| line.strip_prefix("/*"))
            }
            _ if cpp => {
                return crate::formats::extract_cpp::cpp_doc_step(line, &mut in_doxygen_block);
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
            None if collected.is_empty()
                && cpp
                && crate::formats::extract_cpp::is_banner_shaped(line) => {}
            // Every other language, and C++ once real code appears: no doc
            // comment opens this file. Past the first doc line, a non-doc
            // line ends the paragraph.
            None => break,
        }
    }
    (!collected.is_empty()).then(|| collected.join("\n"))
}
