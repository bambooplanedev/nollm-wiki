//! For the `wiki` crate only. Not a contract; may change in any release.
//!
//! Small text and path helpers shared by the wiki and the code layer.

use std::path::Path;

pub use crate::formats::code::{
    ext_of, is_cpp, is_shell, module_stem, source_rank, validate_queries, CODE_EXTENSIONS,
};
pub use crate::summary::{first_real_sentence, summarize};

/// The words of an identifier, lowercased: split on `_`/`-`, at a
/// lower→Upper boundary, and at an Upper→Upper-lower boundary so an
/// acronym run stays one word (`HTTPServer` → `http`, `server`;
/// `PackBudget` → `pack`, `budget`; `CACHE_VERSION` → `cache`,
/// `version`). Must run on the ORIGINAL-case name: lowercasing first
/// erases every CamelCase boundary.
pub fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let boundary = c.is_uppercase()
            && i > 0
            && (chars[i - 1].is_lowercase()
                || (chars[i - 1].is_uppercase()
                    && chars.get(i + 1).is_some_and(|n| n.is_lowercase())));
        if boundary && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// True if `path` lies within `dir`, compared on lexically-absolute components
/// (`std::path::absolute` — no filesystem access, no symlink resolution). This
/// is a predicate only: it never reaches output bytes, so it stays
/// deterministic across machines. Falls back to a raw component compare if
/// absolute-normalization errors (degrade, never panic).
pub fn is_under(path: &Path, dir: &Path) -> bool {
    match (std::path::absolute(path), std::path::absolute(dir)) {
        (Ok(p), Ok(d)) => p.starts_with(d),
        _ => path.starts_with(dir),
    }
}

/// The last `/`-separated segment of `path`.
pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `title_case` of a file stem or directory name, `_` and `-` read as spaces.
pub fn title_from_slug(s: &str) -> String {
    title_case(&s.replace(['_', '-'], " "))
}

/// `s` with every whitespace run, newlines included, collapsed to one space,
/// and with ANSI escape sequences, other control characters and bidi format
/// characters dropped (a tool log's colours must not reach the terminal).
pub fn one_line(s: &str) -> String {
    let mut clean = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                // A CSI sequence runs to its first final byte; `[` itself is
                // in that range, so the scan starts after it.
                if chars.as_str().starts_with('[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
            }
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => {}
            c if c.is_control() && !c.is_whitespace() => {}
            c => clean.push(c),
        }
    }
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The most chars of an item's whole doc kept in `items.json`, and of a
/// summary printed by the page view.
pub const ITEM_DOC_TEXT_MAX_CHARS: usize = 1000;

/// `one_line(s)` cut to `chars` characters, the last one `…` when cut.
pub fn clip(s: &str, chars: usize) -> String {
    let line = one_line(s);
    if line.chars().count() > chars {
        let mut cut: String = line.chars().take(chars - 1).collect();
        cut.push('…');
        cut
    } else {
        line
    }
}

/// Capitalize each whitespace-separated word and lowercase the rest of it.
/// Shared by every name-deriving path — the text extractor's ALL-CAPS first
/// line, the markdown/code filename fallbacks, and `disambiguate_ids`'
/// directory prefixes — so that a page named from a path and a page named
/// from its own content are cased by one rule rather than three copies of it.
pub fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &ch.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Repo-relative, forward-slash-normalized path string. Falls back to the raw
/// path (lossy) if `path` is not under `root`.
pub fn normalize_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_drops_control_characters_and_ansi_sequences() {
        assert_eq!(one_line("a \x1b[31mred\x1b[0m b\x07c\x00d"), "a red bcd");
        assert_eq!(one_line("x\x1b[0;37my"), "xy");
        assert_eq!(one_line("left\u{202E}right\u{2066}x\u{2069}"), "leftrightx");
        assert_eq!(one_line("[0;37m é 日本 🚀\t\n  z"), "[0;37m é 日本 🚀 z");
        assert_eq!(one_line("cr\rtail"), "cr tail");
    }

    #[test]
    fn normalize_path_is_relative_and_forward_slashed() {
        let root = Path::new("/tmp/proj");
        let p = Path::new("/tmp/proj/src/graph.rs");
        assert_eq!(normalize_path(root, p), "src/graph.rs");
    }
}
