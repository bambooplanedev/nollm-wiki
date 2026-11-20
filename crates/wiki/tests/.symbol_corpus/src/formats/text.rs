//! Plain-text extractor: `# Title` or an ALL-CAPS first line, `created:`/`aliases:` header fields, then the body.

use crate::formats::{derive_name_from_path, summarize, Extractor};
use crate::model::{slugify, title_case, Entity, SourceKind};
use regex::Regex;
use std::sync::LazyLock;

static HEADER_HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#\s*(.+)$").unwrap());
static CREATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^created:\s*(.+)$").unwrap());
static ALIASES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^aliases:\s*(.+)$").unwrap());

pub struct TextExtractor;

impl Extractor for TextExtractor {
    fn extensions(&self) -> &[&str] {
        &["txt"]
    }

    fn extract(&self, rel_path: &str, text: &str) -> Entity {
        let mut name: Option<String> = None;
        let mut aliases: Vec<String> = Vec::new();
        let mut created = String::new();
        let mut body_lines: Vec<&str> = Vec::new();

        for (idx, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() {
                body_lines.push(raw_line);
                continue;
            }
            if name.is_none() {
                if let Some(c) = HEADER_HASH.captures(line) {
                    name = Some(c[1].trim().to_string());
                    continue;
                }
                if idx == 0 && line.chars().any(char::is_alphabetic) && line == line.to_uppercase()
                {
                    name = Some(title_case(line));
                    continue;
                }
            }
            if let Some(c) = CREATED.captures(line) {
                created = c[1].trim().to_string();
                continue;
            }
            if let Some(c) = ALIASES.captures(line) {
                aliases = c[1]
                    .split(',')
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty())
                    .collect();
                continue;
            }
            body_lines.push(raw_line);
        }

        let name = name.unwrap_or_else(|| derive_name_from_path(rel_path));
        let body = body_lines.join("\n").trim().to_string();
        let summary = summarize(None, None, &body, None);

        Entity {
            id: slugify(&name),
            name,
            aliases,
            created,
            body,
            source_path: String::new(), // filled by Registry
            kind: SourceKind::Text,
            content_hash: [0u8; 32], // filled by Registry
            summary,
            symbols: Vec::new(),
            imports: Vec::new(),
            defined: Vec::new(),
            methods: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::Registry;

    #[test]
    fn text_pages_define_no_names() {
        let e = TextExtractor.extract("a.txt", "# A\n\nSome prose.\n");
        assert!(e.defined.is_empty(), "defined: {:?}", e.defined);
    }

    #[test]
    fn hash_header_extracted() {
        let e = TextExtractor.extract(
            "a.txt",
            "# My Topic\ncreated: 2026-01-01\n\nbody text here\n",
        );
        assert_eq!(e.name, "My Topic");
        assert_eq!(e.id, "my_topic");
        assert_eq!(e.created, "2026-01-01");
        assert!(e.body.contains("body text here"));
    }

    #[test]
    fn bare_uppercase_header_extracted() {
        let e = TextExtractor.extract("b.txt", "MY TOPIC\n\nsome content\n");
        assert_eq!(e.name, "My Topic");
    }

    #[test]
    fn missing_header_falls_back_to_filename() {
        let e = TextExtractor.extract("fallback_name.txt", "just some prose, no header\n");
        assert_eq!(e.name, "Fallback Name");
        let e = TextExtractor.extract("notes/my-notes.txt", "just some prose, no header\n");
        assert_eq!(e.name, "My Notes");
    }

    #[test]
    fn aliases_parsed() {
        let e = TextExtractor.extract("c.txt", "# Thing\naliases: t1, t2, t3\n\nbody\n");
        assert_eq!(e.aliases, vec!["t1", "t2", "t3"]);
    }

    #[test]
    fn registry_fills_source_path_and_hash_and_skips_unknown_ext() {
        let reg = Registry::with_defaults();
        let e = reg.extract("notes/x.txt", b"# X\n\nbody\n").unwrap();
        assert_eq!(e.source_path, "notes/x.txt");
        assert_ne!(e.content_hash, [0u8; 32]);
        assert!(reg.extract("image.png", b"\x89PNG").is_none());
    }
}
