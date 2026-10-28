//! Deterministic, no-LLM one-line summaries via a fallback chain.

/// First non-empty of: frontmatter description, docstring, the body's first
/// sentence, the first signature.
pub fn summarize(
    frontmatter_desc: Option<&str>,
    docstring: Option<&str>,
    body: &str,
    first_signature: Option<&str>,
) -> Option<String> {
    if let Some(d) = non_empty(frontmatter_desc) {
        return Some(first_sentence(d));
    }
    if let Some(d) = docstring.and_then(first_real_sentence) {
        return Some(d);
    }
    if let Some(s) = first_real_sentence(body) {
        return Some(s);
    }
    non_empty(first_signature).map(std::string::ToString::to_string)
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

fn is_boilerplate(line: &str) -> bool {
    let l = line.trim_start().to_lowercase();
    l.starts_with("this document")
        || l.starts_with("this page")
        || l.starts_with("this module")
        || l.starts_with("this file")
        || l.starts_with("this note")
}

/// A Doxygen or `JSDoc` block tag line (`@param id …`, `\return …`).
fn is_doc_tag(line: &str) -> bool {
    let mut chars = line.chars();
    matches!(chars.next(), Some('@' | '\\'))
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// First non-heading, non-boilerplate sentence from a text block.
///
/// The sentence is taken from the whole paragraph that opens with the first
/// real line — consecutive non-empty lines joined by one space — so a
/// sentence wrapped at 80 columns (a `//!` block, README prose) is not cut at
/// the line break. A tag line ends the paragraph: a `@brief` with no period
/// would otherwise run on into the `@param` lines under it. A list item ends
/// it too, and a list's summary is its first item, not the whole list joined.
pub(crate) fn first_real_sentence(text: &str) -> Option<String> {
    let mut lines = text.lines().map(str::trim);
    let first = lines.find(|l| !l.is_empty() && !l.starts_with('#') && !is_boilerplate(l))?;
    let first = list_item_text(first).unwrap_or(first);
    let paragraph = std::iter::once(first)
        .chain(lines.take_while(|l| !l.is_empty() && !is_doc_tag(l) && !is_list_item(l)))
        .collect::<Vec<_>>()
        .join(" ");
    Some(first_sentence(&paragraph))
}

/// The text after a list marker (`- `, `* `, `+ `, `12. `) of a trimmed line.
fn list_item_text(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix(['-', '*', '+']) {
        return rest.strip_prefix(' ');
    }
    line.trim_start_matches(|c: char| c.is_ascii_digit())
        .strip_prefix(". ")
        .filter(|_| line.starts_with(|c: char| c.is_ascii_digit()))
}

fn is_list_item(line: &str) -> bool {
    list_item_text(line).is_some()
}

/// Truncate to the first sentence terminator (`.`/`!`/`?`), inclusive; else
/// whole line. A terminator counts only when followed by whitespace or the
/// end of the line, so `index.json` or `v1.2` inside a sentence does not end
/// it, and neither does the dot of an initialism (`e.g.`, `i.e.`, `U.S.`).
fn first_sentence(line: &str) -> String {
    let line = line.trim();
    let end = line
        .char_indices()
        .find(|&(i, c)| {
            matches!(c, '.' | '!' | '?')
                && line[i + 1..].chars().next().is_none_or(char::is_whitespace)
                && !ends_in_initialism(&line[..=i])
        })
        .map(|(i, _)| i);
    match end {
        Some(idx) => line[..=idx].trim().to_string(),
        None => line.to_string(),
    }
}

/// `text` ends in two or more single letters each followed by a dot.
fn ends_in_initialism(text: &str) -> bool {
    let word = text.rsplit(char::is_whitespace).next().unwrap_or("");
    let word = word.trim_start_matches(|c: char| !c.is_alphabetic());
    let pairs: Vec<&str> = word.split_inclusive('.').collect();
    pairs.len() >= 2
        && pairs.iter().all(|p| {
            let mut cs = p.chars();
            cs.next().is_some_and(char::is_alphabetic) && cs.as_str() == "."
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_frontmatter_description() {
        let s = summarize(Some("Short desc."), None, "body sentence.", None);
        assert_eq!(s.as_deref(), Some("Short desc."));
    }

    #[test]
    fn skips_headings_and_boilerplate_first_sentence() {
        let body = "# Title\nThis document describes stuff.\nGradient descent optimizes loss.";
        let s = summarize(None, None, body, None);
        assert_eq!(s.as_deref(), Some("Gradient descent optimizes loss."));
    }

    #[test]
    fn a_dot_inside_a_word_does_not_end_the_sentence() {
        let s = summarize(
            None,
            Some("Renders index.json and llms.txt for agents. Second sentence."),
            "",
            None,
        );
        assert_eq!(
            s.as_deref(),
            Some("Renders index.json and llms.txt for agents.")
        );
        assert_eq!(first_sentence("Bump to v1.2"), "Bump to v1.2");
        assert_eq!(first_sentence("Done!Next"), "Done!Next");
    }

    #[test]
    fn an_initialism_does_not_end_the_sentence() {
        assert_eq!(
            first_sentence("Price in admin tables, e.g. 12.50 EUR. Uses a dot."),
            "Price in admin tables, e.g. 12.50 EUR."
        );
        assert_eq!(
            first_sentence("Ships to the U.S. only. Rest."),
            "Ships to the U.S. only."
        );
        assert_eq!(first_sentence("Read a file. Then go."), "Read a file.");
        assert_eq!(first_sentence("Use plan B. Then go."), "Use plan B.");
        assert_eq!(
            first_sentence("Logs, caches, etc. Then go."),
            "Logs, caches, etc."
        );
        // A sentence that really ends on an initialism runs on: after `e.g.` a
        // capital is as common as after a real end, so the two look the same.
        assert_eq!(
            first_sentence("Made in the U.S. Next one."),
            "Made in the U.S. Next one."
        );
    }

    #[test]
    fn a_tag_line_ends_the_first_sentence() {
        let doc =
            "Pin the current thread to a core\n@param coreId The core\n@return true on success";
        let s = summarize(None, Some(doc), "", None);
        assert_eq!(s.as_deref(), Some("Pin the current thread to a core"));
        let doc = "Async write callback\n\\param ec The status code";
        let s = summarize(None, Some(doc), "", None);
        assert_eq!(s.as_deref(), Some("Async write callback"));
        // An `@` that opens no tag stays in the paragraph.
        let doc = "Mails the owner\n@ 9am daily";
        let s = summarize(None, Some(doc), "", None);
        assert_eq!(s.as_deref(), Some("Mails the owner @ 9am daily"));
    }

    #[test]
    fn a_sentence_wrapped_across_lines_is_joined_before_the_cut() {
        let doc = "Rust extraction: bare-`pub` gating, owner qualification through\nimpl scopes. Second sentence\nwraps too.";
        assert_eq!(
            summarize(None, Some(doc), "", None).as_deref(),
            Some("Rust extraction: bare-`pub` gating, owner qualification through impl scopes.")
        );
        // The paragraph ends at a blank line: a following paragraph is never
        // glued on when the first has no terminator.
        let body = "first paragraph without a stop\n\nSecond paragraph.";
        assert_eq!(
            summarize(None, None, body, None).as_deref(),
            Some("first paragraph without a stop")
        );
    }

    #[test]
    fn falls_back_to_signature_then_none() {
        assert_eq!(
            summarize(None, None, "", Some("fn foo(a: i32)")).as_deref(),
            Some("fn foo(a: i32)")
        );
        assert_eq!(summarize(None, None, "   \n\n", None), None);
    }

    #[test]
    fn a_list_yields_its_first_item() {
        let s = summarize(
            None,
            None,
            "- [A](a.md) — first\n- [B](b.md) — second",
            None,
        );
        assert_eq!(s.as_deref(), Some("[A](a.md) — first"));
    }

    #[test]
    fn a_list_item_ends_the_paragraph() {
        let s = summarize(None, None, "Intro line\n- item", None);
        assert_eq!(s.as_deref(), Some("Intro line"));
    }

    #[test]
    fn numbered_items() {
        let s = summarize(None, None, "1. one\n2. two", None);
        assert_eq!(s.as_deref(), Some("one"));
        let s = summarize(None, None, "10. ten\n11. eleven", None);
        assert_eq!(s.as_deref(), Some("ten"));
    }

    #[test]
    fn not_list_items() {
        for (body, want) in [
            ("-foo bar\nmore.", "-foo bar more."),
            ("*emphasis* text\nnext.", "*emphasis* text next."),
            ("Intro\n---\nmore.", "Intro --- more."),
        ] {
            assert_eq!(summarize(None, None, body, None).as_deref(), Some(want));
        }
    }

    #[test]
    fn wrapped_item_continues() {
        let s = summarize(None, None, "- first item\n  wraps here\n- second", None);
        assert_eq!(s.as_deref(), Some("first item wraps here"));
    }
}
