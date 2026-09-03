//! Markdown extractor: `---` frontmatter (title, created, description, aliases), first `# H1` as a title fallback, then the body.

use crate::formats::{derive_name_from_path, summarize, Extractor};
use crate::model::{slugify, Entity, SourceKind};

pub struct MarkdownExtractor;

impl Extractor for MarkdownExtractor {
    fn extensions(&self) -> &[&str] {
        &["md", "markdown"]
    }

    fn extract(&self, rel_path: &str, text: &str) -> Entity {
        let (fm, body_text) = split_frontmatter(text);

        let mut title = fm_get(&fm, "title");
        let created = fm_get(&fm, "created").unwrap_or_default();
        let description = fm_get(&fm, "description");
        let aliases = fm_get(&fm, "aliases")
            .map(|v| parse_list(&v))
            .unwrap_or_default();

        if title.is_none() {
            title = first_h1(body_text);
        }
        let name = title.unwrap_or_else(|| derive_name_from_path(rel_path));
        let body = body_text.trim().to_string();
        let summary = summarize(description.as_deref(), None, &body, None);

        Entity {
            id: slugify(&name),
            name,
            aliases,
            created,
            body,
            source_path: String::new(),
            kind: SourceKind::Markdown,
            content_hash: [0u8; 32],
            summary,
            symbols: Vec::new(),
            imports: Vec::new(),
            defined: Vec::new(),
            methods: Vec::new(),
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

fn fm_get(fm: &[String], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    fm.iter()
        .find_map(|l| l.trim().strip_prefix(&prefix).map(|v| v.trim().to_string()))
        .filter(|v| !v.is_empty())
}

fn parse_list(v: &str) -> Vec<String> {
    let v = v.trim().trim_start_matches('[').trim_end_matches(']');
    v.split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn first_h1(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("# ").map(|h| h.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_pages_define_no_names() {
        let e = MarkdownExtractor.extract("a.md", "# A\n\nSome prose about Beta.\n");
        assert!(e.defined.is_empty(), "defined: {:?}", e.defined);
    }

    #[test]
    fn frontmatter_fields_parsed() {
        let src = "---\ntitle: My Note\naliases: foo, bar\ncreated: 2026-02-02\ndescription: A short desc.\n---\n\nBody line one.\n";
        let e = MarkdownExtractor.extract("n.md", src);
        assert_eq!(e.name, "My Note");
        assert_eq!(e.aliases, vec!["foo", "bar"]);
        assert_eq!(e.created, "2026-02-02");
        assert_eq!(e.summary.as_deref(), Some("A short desc."));
        assert!(e.body.starts_with("Body line one."));
    }

    #[test]
    fn h1_fallback_when_no_frontmatter_title() {
        let e = MarkdownExtractor.extract("n.md", "# Heading Title\n\nprose here.\n");
        assert_eq!(e.name, "Heading Title");
    }

    #[test]
    fn filename_fallback_when_no_title() {
        let e = MarkdownExtractor.extract("my_file.md", "just prose\n");
        assert_eq!(e.name, "My File");
    }

    #[test]
    fn crlf_frontmatter_body_is_clean() {
        // Same content as frontmatter_fields_parsed, but CRLF line endings.
        let src = "---\r\ntitle: My Note\r\naliases: foo, bar\r\ncreated: 2026-02-02\r\n---\r\n\r\nBody line one.\r\nBody line two.\r\n";
        let e = MarkdownExtractor.extract("n.md", src);
        assert_eq!(e.name, "My Note");
        assert_eq!(e.aliases, vec!["foo", "bar"]);
        assert_eq!(e.created, "2026-02-02");
        assert!(
            e.body.starts_with("Body line one."),
            "body should start at content, got: {:?}",
            e.body
        );
        assert!(
            !e.body.contains("---"),
            "closing delimiter leaked into body: {:?}",
            e.body
        );
    }
}
