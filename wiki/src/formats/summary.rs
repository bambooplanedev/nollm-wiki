/// Deterministic, no-LLM one-line summary via a fallback chain.
pub fn summarize(
    frontmatter_desc: Option<&str>,
    docstring: Option<&str>,
    body: &str,
    first_signature: Option<&str>,
) -> Option<String> {
    if let Some(d) = non_empty(frontmatter_desc) {
        return Some(first_sentence(d));
    }
    if let Some(d) = docstring.and_then(|d| first_real_sentence(d)) {
        return Some(d);
    }
    if let Some(s) = first_real_sentence(body) {
        return Some(s);
    }
    non_empty(first_signature).map(|s| s.to_string())
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

/// First non-heading, non-boilerplate sentence from a text block.
fn first_real_sentence(text: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || is_boilerplate(line) {
            continue;
        }
        return Some(first_sentence(line));
    }
    None
}

/// Truncate to the first sentence terminator (`.`/`!`/`?`), inclusive; else whole line.
fn first_sentence(line: &str) -> String {
    let line = line.trim();
    if let Some(idx) = line.find(['.', '!', '?']) {
        line[..=idx].trim().to_string()
    } else {
        line.to_string()
    }
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
    fn falls_back_to_signature_then_none() {
        assert_eq!(
            summarize(None, None, "", Some("fn foo(a: i32)")).as_deref(),
            Some("fn foo(a: i32)")
        );
        assert_eq!(summarize(None, None, "   \n\n", None), None);
    }
}
