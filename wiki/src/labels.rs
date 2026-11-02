//! Markdown labels: ATX headings and bold step lines, each with the line range it covers.

use crate::model::{clip, LINE_CHARS};
use crate::outline::heading;
use std::path::Path;

/// Level given to a bold label, below every heading level.
const BOLD: u8 = 7;

/// A heading or bold step line and the 1-based inclusive lines it spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub start: usize,
    pub end: usize,
    /// 1-6 for an ATX heading, 7 for a bold label.
    pub level: u8,
    pub text: String,
}

/// The text of a bold label line: after leading spaces and an optional list
/// marker it starts with `**` and closes it. A text ending in `:` is a field.
fn bold(line: &str) -> Option<String> {
    let mut s = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        s = s.strip_prefix(marker).unwrap_or(s);
    }
    if let Some((n, rest)) = s.split_once(". ") {
        if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) {
            s = rest;
        }
    }
    let text = s.strip_prefix("**")?.split_once("**")?.0.trim();
    (!text.is_empty() && !text.ends_with(':')).then(|| text.to_string())
}

/// Headings and bold labels of `md` in document order, skipping fenced code.
/// A label ends on the line before the next label of the same or a higher
/// level, else on the last line.
pub fn labels(md: &str) -> Vec<Label> {
    let mut out: Vec<Label> = Vec::new();
    // The open fence as (char, length); it closes on a line of at least as many of that char.
    let mut fence: Option<(char, usize)> = None;
    for (i, line) in md.lines().enumerate() {
        let t = line.trim_start();
        let c = t.chars().next().filter(|c| matches!(c, '`' | '~'));
        let run = c.map_or(0, |c| t.chars().take_while(|&x| x == c).count());
        match (fence, c) {
            (Some((fc, n)), Some(c)) if c == fc && run >= n && t[run..].trim().is_empty() => {
                fence = None;
                continue;
            }
            (Some(_), _) => continue,
            (None, Some(c)) if run >= 3 => {
                fence = Some((c, run));
                continue;
            }
            _ => {}
        }
        let found = heading(line)
            .filter(|(_, t)| !t.is_empty())
            .map(|(l, t)| (l as u8, t))
            .or_else(|| bold(line).map(|t| (BOLD, t)));
        if let Some((level, text)) = found {
            out.push(Label {
                start: i + 1,
                end: 0,
                level,
                text,
            });
        }
    }
    let last = md.lines().count();
    for i in 0..out.len() {
        let level = out[i].level;
        out[i].end = out[i + 1..]
            .iter()
            .find(|n| n.level <= level)
            .map_or(last, |n| n.start - 1);
    }
    out
}

/// Hits returned for a query with words when no `limit` is given.
pub const DOC_LIMIT: usize = 10;

/// A label found in one of the searched documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocHit {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Lowercased words: split on anything but alphanumerics, `-`, `_`, `.`;
/// trailing `.` and `-` trimmed.
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .map(|w| w.trim_end_matches(['.', '-']).to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Labels of the markdown files `docs` (relative to `root`) whose text holds
/// the query's words consecutively. Headings rank before bold labels, a match
/// at the first word before a later one, then `docs` order, then document
/// order. Returns the hits and one error line per unusable path.
pub fn doc_search(
    root: &Path,
    docs: &[String],
    query: &str,
    limit: Option<usize>,
) -> (Vec<DocHit>, Vec<String>) {
    let q = words(query);
    let mut ranked = Vec::new();
    let mut errors = Vec::new();
    for path in docs {
        if !Path::new(path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            errors.push(format!("{path}: not a markdown file"));
            continue;
        }
        let Ok(md) = std::fs::read_to_string(root.join(path)) else {
            errors.push(format!("{path}: not found"));
            continue;
        };
        for l in labels(&md) {
            let w = words(&l.text);
            let at = if q.is_empty() {
                Some(0)
            } else {
                w.windows(q.len()).position(|win| win == q.as_slice())
            };
            if let Some(at) = at {
                let hit = DocHit {
                    path: path.clone(),
                    start: l.start,
                    end: l.end,
                    text: l.text,
                };
                ranked.push(((l.level == BOLD, at > 0), hit));
            }
        }
    }
    if !q.is_empty() {
        ranked.sort_by_key(|(k, _)| *k);
    }
    let n = if q.is_empty() {
        limit.unwrap_or(usize::MAX)
    } else {
        limit.unwrap_or(DOC_LIMIT)
    };
    let hits = ranked.into_iter().map(|(_, h)| h).take(n).collect();
    (hits, errors)
}

/// `path:start-end  text` per hit, then the error lines; `no matches` if none.
pub fn doc_lines(hits: &[DocHit], errors: &[String]) -> String {
    let mut out = String::new();
    for h in hits {
        out.push_str(&format!(
            "{}:{}-{}  {}\n",
            h.path,
            h.start,
            h.end,
            clip(&h.text, LINE_CHARS)
        ));
    }
    for e in errors {
        out.push_str(e);
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("no matches\n");
    }
    out
}
