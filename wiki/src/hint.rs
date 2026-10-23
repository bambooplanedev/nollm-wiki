//! `wiki hint`: the code locations an agent hook injects before it reads a task.

use crate::model::KindFilter;
use crate::query::Wiki;
use std::collections::HashSet;

/// Most symbol lines a hint prints.
pub const HINT_LINES: usize = 8;
/// Code pages searched for a hint.
pub const HINT_PAGES: usize = 5;
/// A page is kept when its score is at least this share of the best page's.
pub const HINT_MIN_REL: f64 = 0.5;
/// Longest line, in characters, before it is cut with `…`.
pub const HINT_LINE_CHARS: usize = 160;

const HEADER: &str = "Code locations from the project wiki (path:start-end  name  signature); read only these lines:\n";

/// Header plus up to `HINT_LINES` `path:start-end  name  signature` lines for
/// `text`, or an empty string when no code page matches.
#[must_use]
pub fn hint_lines(wiki: &Wiki, text: &str) -> String {
    let hits = wiki.search(text, Some(&KindFilter::Code), HINT_PAGES);
    let best = hits.iter().map(|h| h.score).fold(0.0, f64::max);
    let mut seen = HashSet::new();
    let mut out = String::new();
    let symbols = hits
        .iter()
        .filter(|h| h.score >= HINT_MIN_REL * best)
        .flat_map(|h| &h.symbols)
        .filter(|s| seen.insert((s.path.clone(), s.start_line, s.end_line)))
        .take(HINT_LINES);
    for s in symbols {
        let line = format!(
            "{}:{}-{}  {}  {}",
            s.path, s.start_line, s.end_line, s.name, s.signature
        );
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.chars().count() > HINT_LINE_CHARS {
            out.extend(line.chars().take(HINT_LINE_CHARS - 1));
            out.push('…');
        } else {
            out.push_str(&line);
        }
        out.push('\n');
    }
    if out.is_empty() {
        out
    } else {
        format!("{HEADER}{out}")
    }
}

/// The `prompt` string of Claude Code's `UserPromptSubmit` hook input.
#[must_use]
pub fn prompt_from_hook_json(json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()?
        .get("prompt")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::prompt_from_hook_json;

    #[test]
    fn prompt_from_hook_json_needs_a_string_prompt() {
        assert_eq!(prompt_from_hook_json("{}"), None);
        assert_eq!(prompt_from_hook_json("not json"), None);
        assert_eq!(prompt_from_hook_json(r#"{"prompt": 3}"#), None);
        assert_eq!(
            prompt_from_hook_json(r#"{"prompt": "hi"}"#),
            Some("hi".into())
        );
    }
}
