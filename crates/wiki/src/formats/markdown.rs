//! Markdown extractor: `---` frontmatter (title, created, description, aliases, tags), first `# H1` as a title fallback, then the body.

use regex::Regex;
use std::collections::BTreeSet;
use std::sync::LazyLock;

use crate::formats::{derive_name_from_path, summarize, Extractor};
use crate::model::{basename, days_in_month, slugify, Entity, SourceKind};

pub struct MarkdownExtractor;

impl Extractor for MarkdownExtractor {
    fn extensions(&self) -> &[&str] {
        &["md", "markdown"]
    }

    fn extract(&self, rel_path: &str, text: &str) -> Entity {
        let (fm, body_text) = split_frontmatter(text);

        let created = fm_get(&fm, "created").unwrap_or_default();
        let updated = find_date(&fm, rel_path);
        let description = fm_get(&fm, "description");
        let aliases = fm_list(&fm, "aliases");
        let tags = normalize_tags(fm_list(&fm, "tags"));
        let supersedes = fm_list(&fm, "supersedes");

        let name = fm_get(&fm, "title")
            .or_else(|| first_h1(body_text))
            .unwrap_or_else(|| derive_name_from_path(rel_path));
        let body = body_text.trim().to_string();
        let summary = summarize(description.as_deref(), None, &body, None);

        Entity {
            id: slugify(&name),
            name,
            aliases,
            tags,
            created,
            updated,
            supersedes,
            body,
            kind: SourceKind::Markdown,
            summary,
            ..Entity::default()
        }
    }
}

/// Returns (frontmatter lines, remaining body). Frontmatter is a leading block
/// delimited by lines containing only `---`. CRLF-safe: byte offsets come from
/// `split_inclusive('\n')`, whose segments retain their `\r\n`, so the body
/// slice lands on a true byte boundary for both `\n` and `\r\n` files.
fn split_frontmatter(text: &str) -> (Vec<String>, &str) {
    let mut segments = text.split_inclusive('\n');
    let Some(first) = segments.next() else {
        return (Vec::new(), text);
    };
    if first.trim() != "---" {
        return (Vec::new(), text);
    }
    let mut consumed = first.len();
    let mut fm = Vec::new();
    for seg in segments {
        consumed += seg.len();
        if seg.trim() == "---" {
            let rest = text.get(consumed..).unwrap_or("");
            return (fm, rest);
        }
        fm.push(seg.trim_end_matches(['\n', '\r']).to_string());
    }
    (Vec::new(), text) // unterminated frontmatter → treat whole file as body
}

const DATE_KEYS: [&str; 3] = ["updated", "modified", "date"];

/// The page's date: the first valid one among the top-level date keys, the same
/// keys one level down in a top-level `metadata:` block, then the file name's stem.
fn find_date(fm: &[String], rel_path: &str) -> String {
    let top = DATE_KEYS
        .iter()
        .find_map(|k| fm_get(fm, k).as_deref().and_then(leading_date));
    let block = || {
        let lines: Vec<&String> = fm
            .iter()
            .skip_while(|l| l.trim_end() != "metadata:")
            .skip(1)
            .take_while(|l| l.starts_with([' ', '\t']))
            .collect();
        let indent = lines.first()?.len() - lines.first()?.trim_start().len();
        let own: Vec<String> = lines
            .iter()
            .filter(|l| l.len() - l.trim_start().len() == indent)
            .map(|l| l.trim_start().to_string())
            .collect();
        DATE_KEYS
            .iter()
            .find_map(|k| fm_get(&own, k).as_deref().and_then(leading_date))
    };
    let stem = || {
        let name = basename(rel_path);
        leading_date(name.rsplit_once('.').map_or(name, |(s, _)| s))
    };
    top.or_else(block).or_else(stem).unwrap_or_default()
}

/// The `YYYY-MM-DD` that starts `s`, when it is a real calendar date not
/// followed by another digit. A time or zone after it is ignored, not converted.
fn leading_date(s: &str) -> Option<String> {
    let date = s.get(..10)?;
    let bytes = date.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        bytes[r.clone()]
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| date[r].parse().ok())?
    };
    if bytes[4] != b'-' || bytes[7] != b'-' || s.as_bytes().get(10).is_some_and(u8::is_ascii_digit)
    {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    (1..=days_in_month(Some(year), month))
        .contains(&day)
        .then(|| date.to_string())
}

/// Value of a top-level `key:` line; indented lines belong to some other key.
fn fm_get(fm: &[String], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    fm.iter()
        .find_map(|l| l.strip_prefix(&prefix).map(unquote))
        .filter(|v| !v.is_empty())
}

/// Front-matter tags as stored: `#` stripped, trimmed, lowercased, empties
/// dropped, duplicates removed keeping the first-seen order.
fn normalize_tags(raw: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in raw {
        let t = t.trim().trim_start_matches('#').trim().to_lowercase();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// A list under a top-level `key:`: inline (`a, b` / `[a, b]`) or a YAML block of `- item` lines.
fn fm_list(fm: &[String], key: &str) -> Vec<String> {
    let prefix = format!("{key}:");
    let Some(i) = fm.iter().position(|l| l.starts_with(&prefix)) else {
        return Vec::new();
    };
    let v = fm[i][prefix.len()..].trim();
    if !v.is_empty() && !v.starts_with('#') {
        return if v == "null" || v == "~" {
            Vec::new()
        } else {
            parse_list(v)
        };
    }
    let mut items = Vec::new();
    for l in fm[i + 1..].iter().map(|l| l.trim()) {
        if l.is_empty() || l.starts_with('#') || l == "-" {
            continue;
        }
        let Some(item) = l.strip_prefix("- ") else {
            break;
        };
        let item = item.trim();
        let item = match item.chars().next().filter(|c| matches!(c, '"' | '\'')) {
            // A quoted item ends at its closing quote; a ` #` inside is text.
            Some(q) => item[1..].find(q).map_or(item, |end| &item[..end + 2]),
            None => item.split(" #").next().unwrap_or(item),
        };
        let item = unquote(item);
        if !item.is_empty() {
            items.push(item);
        }
    }
    items
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let b = s.as_bytes();
    let pair = b.len() >= 2 && matches!(b[0], b'"' | b'\'') && b[b.len() - 1] == b[0];
    if !pair {
        return s.to_string();
    }
    let inner = s[1..s.len() - 1].trim();
    if b[0] == b'\'' {
        return inner.replace("''", "'");
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(&n) if c == '\\' && matches!(n, '"' | '\\') => {
                out.push(n);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

fn parse_list(v: &str) -> Vec<String> {
    let v = v.trim().trim_start_matches('[').trim_end_matches(']');
    v.split(',')
        .map(unquote)
        .filter(|s| !s.is_empty())
        .collect()
}

/// The first `# H1`, cleaned for a title: images dropped (a linked image
/// leaves an empty link, dropped too), links reduced to their text, code
/// spans and escapes untouched. A `[text][ref]` form counts only when the
/// document defines `[ref]: ...`. `None` when nothing is left.
fn first_h1(body: &str) -> Option<String> {
    static IMAGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(`[^`]*`|\\.)|!\[([^\]]*)\](?:\([^)]*\)|\[([^\]]*)\])").unwrap()
    });
    static LINK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(`[^`]*`|\\.)|\[([^\]]*)\](?:\([^)]*\)|\[([^\]]*)\])").unwrap()
    });
    static DEF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^ {0,3}\[([^\]]+)\]:").unwrap());
    let h = body
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("# "))?;
    let norm = |l: &str| {
        l.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let defs: BTreeSet<String> = DEF.captures_iter(body).map(|c| norm(&c[1])).collect();
    // Keeps the match unless it is a code span/escape or a link whose reference is defined.
    let pass = |re: &Regex, text_group: usize, h: &str, keep_text: bool| {
        re.replace_all(h, |c: &regex::Captures| {
            let whole = c[0].to_string();
            if c.get(1).is_some() {
                return whole;
            }
            let text = c.get(text_group).map_or("", |m| m.as_str());
            let label = c.get(text_group + 1);
            match label {
                Some(l)
                    if !defs.contains(&norm(if l.as_str().is_empty() {
                        text
                    } else {
                        l.as_str()
                    })) =>
                {
                    whole
                }
                _ if keep_text => text.to_string(),
                _ => String::new(),
            }
        })
        .into_owned()
    };
    let h = pass(&IMAGE, 2, h, false);
    let h = pass(&LINK, 2, &h, true);
    let h = h.split_whitespace().collect::<Vec<_>>().join(" ");
    (!h.is_empty()).then_some(h)
}

#[cfg(test)]
mod tests;
