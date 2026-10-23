//! `wiki hint`: the code locations an agent hook injects before it reads a task.

use crate::model::KindFilter;
use crate::query::{SymbolLine, Wiki};
use std::collections::HashSet;

/// Most symbol lines a hint prints.
pub const HINT_LINES: usize = 8;
/// Code pages searched for a hint.
pub const HINT_PAGES: usize = 5;
/// A page is kept when its score is at least this share of the best page's.
pub const HINT_MIN_REL: f64 = 0.5;
/// Longest line, in characters, before it is cut with `…`.
pub const HINT_LINE_CHARS: usize = 160;
/// Lines per page, so a fifth page still shows (on the bench the 8 lines
/// ran out on three pages and the answer's file was the fifth).
pub const HINT_PAGE_LINES: usize = 2;

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
        .flat_map(|h| page_lines(&h.symbols))
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
/// A page's lines worth injecting: no test items, no line whose range holds
/// another of the page's lines (a class around its method is thousands of
/// lines no model reads), at most `HINT_PAGE_LINES`.
fn page_lines(symbols: &[SymbolLine]) -> impl Iterator<Item = &SymbolLine> {
    let holds = |a: &SymbolLine, b: &SymbolLine| {
        !std::ptr::eq(a, b) && a.start_line <= b.start_line && b.end_line <= a.end_line
    };
    symbols
        .iter()
        .filter(move |a| !a.test && !symbols.iter().any(|b| !b.test && holds(a, b)))
        .take(HINT_PAGE_LINES)
}

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
    use super::{page_lines, prompt_from_hook_json};
    use crate::query::SymbolLine;

    fn line(name: &str, start: usize, end: usize, test: bool) -> SymbolLine {
        SymbolLine {
            path: "a.h".into(),
            start_line: start,
            end_line: end,
            name: name.into(),
            signature: String::new(),
            doc: None,
            test,
        }
    }

    #[test]
    fn page_lines_drop_tests_and_containers_and_keep_two() {
        let names = |s: &[SymbolLine]| page_lines(s).map(|l| l.name.clone()).collect::<Vec<_>>();
        // The class holds both methods; the test item is dropped.
        let page = [
            line("Fsm", 144, 4487, false),
            line("Fsm::ctor", 157, 212, false),
            line("Fsm::onOrderReject", 1471, 1690, false),
        ];
        assert_eq!(names(&page), ["Fsm::ctor", "Fsm::onOrderReject"]);
        let page = [
            line("decide_test", 640, 650, true),
            line("decide", 76, 107, false),
            line("gate", 200, 220, false),
            line("fetch", 300, 320, false),
        ];
        assert_eq!(names(&page), ["decide", "gate"]);
        // A container whose only inner line is a test item stays.
        let page = [line("mod", 1, 900, false), line("t", 800, 810, true)];
        assert_eq!(names(&page), ["mod"]);
    }

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
