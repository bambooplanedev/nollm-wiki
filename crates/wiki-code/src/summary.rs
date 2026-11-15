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
pub fn first_real_sentence(text: &str) -> Option<String> {
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
mod tests;
