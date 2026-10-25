//! Markdown extractor: `---` frontmatter (title, created, description, aliases, tags), first `# H1` as a title fallback, then the body.

use crate::formats::{derive_name_from_path, normalize_tags, summarize, Extractor};
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
        let aliases = fm_list(&fm, "aliases");
        let tags = normalize_tags(fm_list(&fm, "tags"));

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
            tags,
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
            items: Vec::new(),
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

/// Value of a top-level `key:` line; indented lines belong to some other key.
fn fm_get(fm: &[String], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    fm.iter()
        .find_map(|l| l.strip_prefix(&prefix).map(|v| v.trim().to_string()))
        .filter(|v| !v.is_empty())
}

/// A list under a top-level `key:`: inline (`a, b` / `[a, b]`) or a YAML block of `- item` lines.
fn fm_list(fm: &[String], key: &str) -> Vec<String> {
    let prefix = format!("{key}:");
    let Some(i) = fm.iter().position(|l| l.starts_with(&prefix)) else {
        return Vec::new();
    };
    let v = fm[i][prefix.len()..].trim();
    if !v.is_empty() && !v.starts_with('#') {
        return if v == "null" || v == "~" {
            Vec::new()
        } else {
            parse_list(v)
        };
    }
    let mut items = Vec::new();
    for l in fm[i + 1..].iter().map(|l| l.trim()) {
        if l.is_empty() || l.starts_with('#') || l == "-" {
            continue;
        }
        let Some(item) = l.strip_prefix("- ") else {
            break;
        };
        let item = item.trim();
        let item = match item.chars().next().filter(|c| matches!(c, '"' | '\'')) {
            // A quoted item ends at its closing quote; a ` #` inside is text.
            Some(q) => item[1..].find(q).map_or(item, |end| &item[..end + 2]),
            None => item.split(" #").next().unwrap_or(item),
        };
        let item = unquote(item);
        if !item.is_empty() {
            items.push(item);
        }
    }
    items
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let b = s.as_bytes();
    let pair = b.len() >= 2 && matches!(b[0], b'"' | b'\'') && b[b.len() - 1] == b[0];
    if pair { s[1..s.len() - 1].trim() } else { s }.to_string()
}

fn parse_list(v: &str) -> Vec<String> {
    let v = v.trim().trim_start_matches('[').trim_end_matches(']');
    v.split(',')
        .map(unquote)
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
    fn tag_list_comment_and_quote_edge_cases() {
        let fm = |s: &str| s.lines().map(String::from).collect::<Vec<_>>();
        let tags = |s: &str| fm_list(&fm(s), "tags");
        assert_eq!(
            tags("tags: # my tags\n  - alpha\n  - beta"),
            vec!["alpha", "beta"]
        );
        assert_eq!(tags("tags: # none"), Vec::<String>::new());
        assert_eq!(
            tags("tags:\n  - \"Issue #42\" # c\n  - Peoples'"),
            vec!["Issue #42", "Peoples'"]
        );
        assert_eq!(tags("tags: [\"a b\", 'c']"), vec!["a b", "c"]);
    }

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

    fn aliases_of(fm: &str) -> Vec<String> {
        MarkdownExtractor
            .extract("n.md", &format!("---\n{fm}---\nbody\n"))
            .aliases
    }

    #[test]
    fn tags_normalized_from_front_matter() {
        let e = MarkdownExtractor.extract(
            "n.md",
            "---\ntags:\n  - '#Project/Wiki'\n  - draft\n  - DRAFT\n---\nb\n",
        );
        assert_eq!(e.tags, vec!["project/wiki", "draft"]);
        let e = MarkdownExtractor.extract("n.md", "---\ntags: [a, '#b']\n---\nb\n");
        assert_eq!(e.tags, vec!["a", "b"]);
    }

    #[test]
    fn aliases_block_list_forms() {
        assert_eq!(aliases_of("aliases:\n  - AI\n  - ШІ\n"), vec!["AI", "ШІ"]);
        assert_eq!(aliases_of("aliases:\n- a\n- b\ntitle: T\n"), vec!["a", "b"]);
        assert_eq!(
            aliases_of("aliases:\n  # group\n  - a\n\n  - b # note\n"),
            vec!["a", "b"]
        );
        assert_eq!(aliases_of("aliases:\n  -\n  - real\n"), vec!["real"]);
        assert_eq!(
            aliases_of("aliases:\n  - \"q1\"\n  - 'q2'\n"),
            vec!["q1", "q2"]
        );
    }

    #[test]
    fn aliases_one_line_forms_and_empties() {
        assert_eq!(aliases_of("aliases: a, b\n"), vec!["a", "b"]);
        assert_eq!(aliases_of("aliases: ['a', \"b\"]\n"), vec!["a", "b"]);
        assert!(aliases_of("aliases: []\n").is_empty());
        assert!(aliases_of("aliases: null\n").is_empty());
        assert!(aliases_of("aliases: ~\n").is_empty());
    }

    #[test]
    fn indented_keys_are_not_top_level() {
        assert!(aliases_of("project:\n  aliases:\n    - inner\n").is_empty());
        let e =
            MarkdownExtractor.extract("n.md", "---\ndescription: |\n  title: oops\n---\n# Real\n");
        assert_eq!(e.name, "Real");
    }

    #[test]
    fn crlf_block_list() {
        let e = MarkdownExtractor.extract(
            "n.md",
            "---\r\naliases:\r\n  - a\r\n  - b\r\n---\r\nbody\r\n",
        );
        assert_eq!(e.aliases, vec!["a", "b"]);
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
