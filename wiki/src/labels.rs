//! Markdown labels: ATX headings and bold step lines, each with the line range it covers.

use crate::model::{clip, LINE_CHARS};
use crate::outline::heading;
use crate::rewrite::mask_fenced_code;
use std::collections::HashMap;
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
    // An ordered-list marker: digits then `. ` or `) `.
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && (s[digits..].starts_with(". ") || s[digits..].starts_with(") ")) {
        s = &s[digits + 2..];
    }
    for box_ in ["[ ] ", "[x] ", "[X] "] {
        s = s.strip_prefix(box_).unwrap_or(s);
    }
    let text = s.strip_prefix("**")?.split_once("**")?.0.trim();
    (!text.is_empty() && !text.ends_with(':')).then(|| text.to_string())
}

/// Headings and bold labels of `md` in document order, skipping fenced code.
/// A label ends on the line before the next label of the same or a higher
/// level, else on the last line.
pub fn labels(md: &str) -> Vec<Label> {
    let mut out: Vec<Label> = Vec::new();
    for (i, line) in mask_fenced_code(md).lines().enumerate() {
        let found = heading(line)
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

/// Labels listed for an empty query when no `limit` is given.
pub const DOC_LIST_LIMIT: usize = 200;

/// The line for an empty `docs` answer: why nothing matched and what to try.
pub const DOC_NO_MATCH: &str =
    "no matches: a label must hold the query words in a row; try fewer words or an ID like T5k, or grep for text inside a doc";

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

/// `words` with every word also split on `-`.
fn split_words(s: &str) -> Vec<String> {
    words(s)
        .iter()
        .flat_map(|w| w.split('-'))
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

fn find(w: &[String], q: &[String]) -> Option<usize> {
    w.windows(q.len()).position(|win| win == q)
}

/// Labels of the markdown files `docs` (relative to `root`) whose text holds
/// the query's words consecutively. Headings rank before bold labels, a match
/// at the first word before a later one, then `docs` order, then document
/// order. Matches that hold only once `-` also splits words (two or more such
/// query words) rank after all the others. When the limit cuts some of the
/// labels one doc repeats under a head (text before the first `: `), the
/// slots they had go to the first and last of them alternately. Returns the hits grouped by doc,
/// one error line per unusable path, and how many matching labels the limit
/// (or an empty query's default cap) cut.
pub fn doc_search(
    root: &Path,
    docs: &[String],
    query: &str,
    limit: Option<usize>,
) -> (Vec<DocHit>, Vec<String>, usize) {
    let q = words(query);
    let qs = split_words(query);
    let mut ranked = Vec::new();
    let mut errors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (doc, path) in docs.iter().enumerate() {
        // The same file under another spelling is read once, at its first place.
        let file = root.join(path);
        if !seen.insert(file.canonicalize().unwrap_or(file)) {
            continue;
        }
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
            let exact = if q.is_empty() {
                Some(0)
            } else {
                find(&words(&l.text), &q)
            };
            let found = match exact {
                Some(at) => Some((false, at)),
                None if qs.len() > 1 => find(&split_words(&l.text), &qs).map(|at| (true, at)),
                None => None,
            };
            if let Some((loose, at)) = found {
                let hit = DocHit {
                    path: path.clone(),
                    start: l.start,
                    end: l.end,
                    text: l.text,
                };
                ranked.push(((loose, l.level == BOLD, at > 0), doc, hit));
            }
        }
    }
    if !q.is_empty() {
        ranked.sort_by_key(|(k, _, _)| *k);
    }
    let n = limit.unwrap_or(if q.is_empty() {
        DOC_LIST_LIMIT
    } else {
        DOC_LIMIT
    });
    let more = ranked.len().saturating_sub(n);
    // Rank picks the survivors; output groups them by doc, rank order within.
    // A group the limit cuts (`N11: …` repeated in a ledger) keeps its slots,
    // filled first, last, second, second-to-last, …
    let mut keep: Vec<bool> = (0..ranked.len()).map(|p| p < n).collect();
    if !q.is_empty() && more > 0 {
        let mut groups: HashMap<_, Vec<usize>> = HashMap::new();
        for (p, (k, doc, h)) in ranked.iter().enumerate() {
            groups.entry((*k, *doc, head(&h.text))).or_default().push(p);
        }
        for m in groups.values() {
            let s = m.iter().filter(|&&p| p < n).count();
            for &p in m {
                keep[p] = false;
            }
            for j in 0..s {
                keep[m[if j % 2 == 0 {
                    j / 2
                } else {
                    m.len() - 1 - j / 2
                }]] = true;
            }
        }
    }
    let mut ranked: Vec<_> = ranked
        .into_iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(r, _)| r)
        .collect();
    ranked.sort_by_key(|(_, doc, _)| *doc);
    let hits = ranked.into_iter().map(|(_, _, h)| h).collect();
    (hits, errors, more)
}

/// What repeated labels share: the text before the first `: ` (or a trailing
/// `:`), trimmed and lowercased; the whole text when there is none or it is
/// empty, so `https://a.com` or `` `T5:a` x `` stay their own heads.
fn head(text: &str) -> String {
    let h = text
        .find(": ")
        .map_or(text.strip_suffix(':').unwrap_or(text), |c| &text[..c])
        .trim();
    if h.is_empty() { text } else { h }.to_lowercase()
}

/// Hits grouped by consecutive path: the path alone on a line, then
/// `  start-end  text` per hit; then the error lines; [`DOC_NO_MATCH`] if
/// nothing else prints.
pub fn doc_lines(hits: &[DocHit], errors: &[String], more: usize) -> String {
    let mut out = String::new();
    let mut path = None;
    for h in hits {
        if path != Some(&h.path) {
            out.push_str(&format!("{}\n", h.path));
            path = Some(&h.path);
        }
        out.push_str(&format!(
            "  {}-{}  {}\n",
            h.start,
            h.end,
            clip(&h.text, LINE_CHARS)
        ));
    }
    if more > 0 {
        let noun = if more == 1 { "label" } else { "labels" };
        out.push_str(&format!("… {more} more {noun}; pass limit to list them\n"));
    }
    for e in errors {
        out.push_str(e);
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str(DOC_NO_MATCH);
        out.push('\n');
    }
    out
}
