//! `wiki hint`: the code locations an agent hook injects before it reads a task.

use crate::model::{clip, LINE_CHARS};
use crate::query::{gate_len, MemoryScope, SymbolLine, Wiki, MEMORY_GATE_RATIO};
use std::collections::HashSet;

/// Most symbol lines a hint prints.
const HINT_LINES: usize = 8;
/// Code pages searched for a hint.
const HINT_PAGES: usize = 5;
/// Rarest prompt words that rank the pages (the bench keyed prompts have up
/// to 30 and rank the same at 24; at 16 they lose two answers).
const HINT_TERMS: usize = 24;
/// A prompt with more distinct words than this is instructions, not a
/// question: a memory note named by one of its words is noise, so the name
/// branch of the memory gate is off for it. Measured 2026-11-01: questions
/// and bench task prompts have 2-48 distinct words, long task prompts 116
/// or more; at the shared gate the long ones showed a note 44 times in 48,
/// about 1 in 50 relevant.
const HINT_SHORT_WORDS: usize = 64;
/// A page is kept when its score is at least this share of the best page's.
const HINT_MIN_REL: f64 = 0.5;
/// No code lines when the best page scores below this share of what the
/// prompt's counted words could give (`PassStats::available`): its words are
/// scattered over the project, nothing names the task. Measured 2026-11-01:
/// every bench task prompt that hinted its answer and every code question of
/// this repo scores 0.59 or more (80 of 80 kept); 162 of 251 subagent briefs
/// whose hint named no file the agent then opened score below (silent, about
/// 460 bytes each), and 26 of 75 briefs whose hint did name one stay.
const HINT_MIN_SHARE: f64 = 0.5;
/// Fewest prompt tokens of two or more characters a hint needs: a reply
/// such as `yes`, `ok`, `так` or `1-2` is not a task, and on the bench each
/// found a test fixture or a note.
const HINT_MIN_TOKENS: usize = 2;
/// Lines per page, so a fifth page still shows (on the bench the 8 lines
/// ran out on three pages and the answer's file was the fifth).
const HINT_PAGE_LINES: usize = 2;

const HEADER: &str = "Code locations from the project wiki (path:start-end  name  signature); read only these lines, and read them before you run grep:\n";

/// Header plus up to `HINT_LINES` `path:start-end  name  signature` lines for
/// `text` (`path  title` for a page with no such line, such as a script
/// whose matching code sits outside any function; none when the best page
/// scores below `HINT_MIN_SHARE` of the prompt's available score), then one `memory: path — summary` line for the best memory note
/// that passes the hint's gate: `search`'s tail gate (`Wiki::passes_gate`)
/// with two differences for a prompt: a name match counts only when the
/// prompt has at most `HINT_SHORT_WORDS` distinct words, the ratio branch
/// of a longer prompt needs a code hit to beat, and the note is scored on
/// the same `HINT_TERMS` words as the code. Empty when neither exists.
#[must_use]
pub fn hint_lines(wiki: &Wiki, text: &str) -> String {
    if is_harness_prompt(text) || Wiki::prompt_tokens(text).len() < HINT_MIN_TOKENS {
        return String::new();
    }
    let (hits, available) = wiki.search_prompt(text, HINT_PAGES, HINT_TERMS, MemoryScope::Without);
    let best = hits.iter().map(|h| h.score).fold(0.0, f64::max);
    let named = best >= HINT_MIN_SHARE * available;
    let mut seen = HashSet::new();
    let mut out = String::new();
    let mut left = HINT_LINES;
    for h in hits
        .iter()
        .filter(|h| named && h.score >= HINT_MIN_REL * best)
    {
        let mut lines = page_lines(&h.symbols).peekable();
        if lines.peek().is_none() && left > 0 {
            push_line(&mut out, &format!("{}  {}", h.path, h.title));
            left -= 1;
        }
        for s in lines.filter(|s| seen.insert((s.path.clone(), s.start_line, s.end_line))) {
            if left == 0 {
                break;
            }
            push_line(
                &mut out,
                &format!(
                    "{}:{}-{}  {}  {}",
                    s.path, s.start_line, s.end_line, s.name, s.signature
                ),
            );
            left -= 1;
        }
    }
    let short = Wiki::query_tokens(text).0.len() <= HINT_SHORT_WORDS;
    let note = wiki
        .search_prompt(text, HINT_PAGES, HINT_TERMS, MemoryScope::Only)
        .0
        .into_iter()
        .find(|h| {
            h.superseded_by.is_none()
                && ((short && wiki.names_page(&h.id, text, gate_len()))
                    || wiki.dated_on(&h.id, text)
                    || ((short || best > 0.0) && h.score >= MEMORY_GATE_RATIO * best))
        });
    if let Some(h) = note {
        let said = h.summary.as_deref().filter(|s| !s.trim().is_empty());
        push_line(
            &mut out,
            &format!("memory: {} — {}", h.path, said.unwrap_or(&h.title)),
        );
    }
    if out.is_empty() {
        out
    } else {
        format!("{HEADER}{out}")
    }
}

/// Whether the harness wrote `text`, not the person: Claude Code hands a
/// `UserPromptSubmit` hook its own turns too (`<task-notification>`, a
/// subagent's hand-back, `<command-name>`, `<bash-stdout>`), each opening
/// with a tag. A hint on them is noise, and the hand-backs run to a
/// thousand words.
fn is_harness_prompt(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<') && t[1..].starts_with(|c: char| c.is_ascii_alphabetic())
}

/// Append `line` whitespace-collapsed and cut to `LINE_CHARS`.
fn push_line(out: &mut String, line: &str) {
    out.push_str(&clip(line, LINE_CHARS));
    out.push('\n');
}

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
    use super::{is_harness_prompt, page_lines, prompt_from_hook_json};
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
    fn harness_prompts_open_with_a_tag() {
        assert!(is_harness_prompt(
            "<task-notification>\n<task-id>a1</task-id>"
        ));
        assert!(is_harness_prompt("  <bash-stdout>ok</bash-stdout>"));
        assert!(!is_harness_prompt("fix the < comparison in parse_order"));
        assert!(!is_harness_prompt("<3 this parser"));
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
