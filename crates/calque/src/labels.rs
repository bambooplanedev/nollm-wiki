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

/// `line` after leading spaces, an optional list marker and a checkbox, and
/// whether a list marker was there.
fn lead(line: &str) -> (&str, bool) {
    let mut s = line.trim_start();
    let n = s.len();
    for marker in ["- ", "* ", "+ "] {
        s = s.strip_prefix(marker).unwrap_or(s);
    }
    // An ordered-list marker: digits then `. ` or `) `.
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && (s[digits..].starts_with(". ") || s[digits..].starts_with(") ")) {
        s = &s[digits + 2..];
    }
    let item = s.len() < n;
    for box_ in ["[ ] ", "[x] ", "[X] "] {
        s = s.strip_prefix(box_).unwrap_or(s);
    }
    (s, item)
}

/// A label text, unless empty or a field (ending in `:`).
fn label_text(t: &str) -> Option<String> {
    let t = t.trim();
    (!t.is_empty() && !t.ends_with(':')).then(|| t.to_string())
}

/// Lines a bold label may run on past its first.
const WRAP: usize = 3;

/// The text of a bold label opening on `lines[0]`: after leading spaces, an
/// optional list marker and checkbox it starts with `**`, closed on that line
/// or on one of the next [`WRAP`] lines of the same block (each line up to the
/// close is not blank, a heading, a list item or bold-led), the lines joined
/// with spaces. A text ending in `:` is a field.
fn bold(lines: &[&str]) -> Option<String> {
    let first = lead(lines[0]).0.strip_prefix("**")?;
    if let Some((t, _)) = first.split_once("**") {
        return label_text(t);
    }
    let mut text = first.trim().to_string();
    for l in lines.iter().skip(1).take(WRAP) {
        let (s, item) = lead(l);
        if l.trim().is_empty() || heading(l).is_some() || item || s.starts_with("**") {
            return None;
        }
        let Some((part, rest)) = l.split_once("**") else {
            text = format!("{text} {}", l.trim());
            continue;
        };
        // The close must be right-flanking, as in CommonMark: `**kwargs` after
        // a space or a `(` opens a span instead.
        let before = part.chars().next_back();
        let after = rest.chars().next();
        let punct = |c: Option<char>| c.is_some_and(|c| c.is_ascii_punctuation());
        let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
        if space(before) || (punct(before) && !(space(after) || punct(after))) {
            return None;
        }
        return label_text(&format!("{text} {}", part.trim()));
    }
    None
}

/// Headings and bold labels of `md` in document order, skipping fenced code.
/// A label ends on the line before the next label of the same or a higher
/// level, else on the last line.
pub fn labels(md: &str) -> Vec<Label> {
    let mut out: Vec<Label> = Vec::new();
    let masked = mask_fenced_code(md);
    let lines: Vec<&str> = masked.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let found = heading(line)
            .map(|(l, t)| (l as u8, t))
            .or_else(|| bold(&lines[i..]).map(|t| (BOLD, t)));
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
    "no matches: no label holds the query words in a row, opens with its leading ID, or holds all its words; try other words or an ID like T5k, or grep for text inside a doc";

/// A label found in one of the searched documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocHit {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Words split on anything but alphanumerics, `-`, `_`, `.`; trailing `.`
/// and `-` trimmed.
fn raw_words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .map(|w| w.trim_end_matches(['.', '-']))
        .filter(|w| !w.is_empty())
}

/// `raw_words`, lowercased.
fn words(s: &str) -> Vec<String> {
    raw_words(s).map(str::to_lowercase).collect()
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

/// Words dropped by the any-order fallback.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "by", "for", "from", "in", "into", "is", "of", "on", "or", "the",
    "to", "via", "with",
];

/// Labels of the markdown files `docs` (relative to `root`) whose text holds
/// the query's words consecutively. Headings rank before bold labels, a match
/// at the first word before a later one, then `docs` order, then document
/// order. Matches that hold only once `-` also splits words (two or more such
/// query words) rank after all the others. When the limit cuts some of the
/// labels one doc repeats under a head (text before the first `: `), the
/// slots they had go to the first and last of them alternately. When a query
/// of two or more words matches nothing, it is read as several labels named
/// one after another (see `several`) when two or more of them are found;
/// else labels whose first or second word is
/// the query's leading word answer if that word has a digit (an ID like `11.4`
/// or `T3a`), else labels holding all its words but stop words (two or more)
/// in any order. Returns the hits grouped
/// by doc, one error line per unusable path, how many matching labels the
/// limit (or an empty query's default cap) cut, and the line naming the
/// fallback that answered.
pub fn doc_search(
    root: &Path,
    docs: &[String],
    query: &str,
    limit: Option<usize>,
) -> (Vec<DocHit>, Vec<String>, usize, Option<String>) {
    let q = words(query);
    let qs = split_words(query);
    let (mut ranked, errors) = matching(root, docs, |t| {
        if q.is_empty() {
            return Some((false, 0));
        }
        match find(&words(t), &q) {
            Some(at) => Some((false, at)),
            None if qs.len() > 1 => find(&split_words(t), &qs).map(|at| (true, at)),
            None => None,
        }
    });
    let mut fallback = None;
    if ranked.is_empty() && q.len() > 1 {
        let (parts, missed) = several(root, docs, query);
        if !parts.is_empty() {
            ranked = parts;
            let missed = if missed.is_empty() {
                String::new()
            } else {
                format!(" (no label opens with {})", missed.join(", "))
            };
            fallback = Some(format!(
                "no label holds the words in a row; labels each part of the query opens, in query order{missed}:"
            ));
        }
    }
    if ranked.is_empty() && q.len() > 1 {
        let mut note = String::new();
        let id = raw_words(query)
            .next()
            .filter(|w| w.contains(|c: char| c.is_ascii_digit()));
        if let Some(id) = id {
            ranked = matching(root, docs, |t| {
                find(&words(t), &q[..1])
                    .filter(|&at| at <= 1)
                    .map(|at| (false, at))
            })
            .0;
            note = format!("labels with the leading ID {id}");
        }
        let need: Vec<_> = qs
            .iter()
            .filter(|w| !STOP_WORDS.contains(&w.as_str()))
            .collect();
        if ranked.is_empty() && need.len() > 1 {
            ranked = matching(root, docs, |t| {
                let w = split_words(t);
                need.iter().all(|n| w.contains(n)).then_some((false, 0))
            })
            .0;
            note = "labels with all the words in any order, stop words dropped".into();
        }
        if !ranked.is_empty() {
            fallback = Some(format!("no label holds the words in a row; {note}:"));
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
    (hits, errors, more, fallback)
}

/// Where the query part `run` names a label of words `w`: a part of two or
/// more words, or one led by an ID (a word with a digit and a letter or `.`;
/// a bare `7` is not one), opens the label at
/// its first word, or the ID at its second (`Task T4`); one plain word must be
/// the whole label (`Pins`).
fn names(w: &[String], run: &[String]) -> Option<usize> {
    let id = run[0].contains(|c: char| c.is_ascii_digit())
        && run[0].contains(|c: char| c.is_alphabetic() || c == '.');
    if run.len() == 1 && !id {
        (w == run).then_some(0)
    } else if w.starts_with(run) {
        Some(0)
    } else if id && w.get(1..)?.starts_with(run) {
        Some(1)
    } else {
        None
    }
}

/// A query naming several labels one after another: from each word, the
/// longest run of words that names a label is one part; a word that starts
/// none is skipped and returned. The labels of the parts, in part order, each
/// listed once; none unless two or more parts list a new label.
fn several(root: &Path, docs: &[String], query: &str) -> (Ranked, Vec<String>) {
    let all = matching(root, docs, |_| Some((false, 0))).0;
    let lw: Vec<Vec<String>> = all.iter().map(|(_, _, h)| words(&h.text)).collect();
    let raw: Vec<&str> = raw_words(query).collect();
    let q = words(query);
    let (mut out, mut missed, mut parts) = (Vec::new(), Vec::new(), 0);
    let mut taken = vec![false; all.len()];
    let mut i = 0;
    while i < q.len() {
        let n = (1..=q.len() - i)
            .rev()
            .find(|&n| lw.iter().any(|w| names(w, &q[i..i + n]).is_some()));
        let Some(n) = n else {
            missed.push(raw[i].to_string());
            i += 1;
            continue;
        };
        let mut part: Vec<_> = (0..all.len())
            .filter_map(|k| names(&lw[k], &q[i..i + n]).map(|at| ((all[k].0 .1, at), k)))
            .filter(|&(_, k)| !taken[k])
            .collect();
        part.sort_unstable();
        parts += usize::from(!part.is_empty());
        for (_, k) in part {
            taken[k] = true;
            out.push(((false, false, false), all[k].1, all[k].2.clone()));
        }
        i += n;
    }
    if parts < 2 {
        out.clear();
    }
    (out, missed)
}

/// Rank key, `docs` index and hit of every label whose text `matches` (giving
/// whether it matched loosely and at which word), plus one error per unusable path.
type Ranked = Vec<((bool, bool, bool), usize, DocHit)>;
fn matching(
    root: &Path,
    docs: &[String],
    matches: impl Fn(&str) -> Option<(bool, usize)>,
) -> (Ranked, Vec<String>) {
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
            if let Some((loose, at)) = matches(&l.text) {
                let key = (loose, l.level == BOLD, at > 0);
                let hit = DocHit {
                    path: path.clone(),
                    start: l.start,
                    end: l.end,
                    text: l.text,
                };
                ranked.push((key, doc, hit));
            }
        }
    }
    (ranked, errors)
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

/// The `fallback` line if any; hits grouped by consecutive path: the path
/// alone on a line, then `  start-end  text` per hit; then the error lines;
/// [`DOC_NO_MATCH`] if nothing else prints.
pub fn doc_lines(
    hits: &[DocHit],
    errors: &[String],
    more: usize,
    fallback: Option<&str>,
) -> String {
    let mut out = fallback.map_or(String::new(), |f| format!("{f}\n"));
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
