//! Markdown labels: ATX headings and bold step lines, each with the line range it covers.

use crate::outline::heading;

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
    let mut fence = false;
    for (i, line) in md.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
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
