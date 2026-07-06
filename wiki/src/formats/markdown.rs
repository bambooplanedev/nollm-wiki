use crate::formats::{summarize, Extractor};
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
        let aliases = fm_get(&fm, "aliases").map(parse_list).unwrap_or_default();

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
        }
    }
}

/// Returns (frontmatter lines, remaining body). Frontmatter is a leading block
/// delimited by lines containing only `---`.
fn split_frontmatter(text: &str) -> (Vec<String>, &str) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (Vec::new(), text);
    }
    let mut fm = Vec::new();
    let mut consumed = text.find('\n').map(|i| i + 1).unwrap_or(text.len());
    for line in text[consumed..].lines() {
        consumed += line.len() + 1;
        if line.trim() == "---" {
            let rest = text.get(consumed..).unwrap_or("");
            return (fm, rest);
        }
        fm.push(line.to_string());
    }
    (Vec::new(), text) // unterminated frontmatter → treat whole file as body
}

fn fm_get(fm: &[String], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    fm.iter()
        .find_map(|l| l.trim().strip_prefix(&prefix).map(|v| v.trim().to_string()))
        .filter(|v| !v.is_empty())
}

fn parse_list(v: String) -> Vec<String> {
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

fn derive_name_from_path(rel_path: &str) -> String {
    let base = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let stem = base.split('.').next().unwrap_or(base);
    stem.replace('_', " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
