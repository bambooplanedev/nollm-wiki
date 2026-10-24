//! Wiki health checks (`wiki lint`, MCP `lint` tool): broken wikilinks, orphan pages and pages too large for a default `neighbors` pack over a compiled output directory.

use crate::model::{slugify, LintReport};
use crate::query::Wiki;
use crate::rewrite::{mask_code, parse_sections};
use regex::Regex;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::LazyLock;

static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[(.+?)\]\]").unwrap());

/// Read all rendered pages (`<id>.md`) from a compiled output directory,
/// skipping the non-page files `index.md` and `AGENTS.md`. Used by the
/// `lint` CLI subcommand and the MCP server's `lint` tool.
pub fn load_compiled_pages(dir: &std::path::Path) -> std::io::Result<BTreeMap<String, String>> {
    let mut pages = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem == "index" || stem == "AGENTS" {
                    continue;
                }
                pages.insert(stem.to_string(), std::fs::read_to_string(&path)?);
            }
        }
    }
    Ok(pages)
}

/// The slug side of a wikilink capture: everything before the first `|`
/// (`[[target|display]]`). A bare `[[x]]` returns `x`.
fn link_target(inner: &str) -> &str {
    inner.split('|').next().unwrap_or(inner)
}

/// The human-readable side of a wikilink capture: everything after the last `|`
/// (or the whole capture if there is no `|`). Used for broken-link reports.
fn link_display(inner: &str) -> &str {
    inner.rsplit('|').next().unwrap_or(inner)
}

/// Whether a rendered page's `## Metadata` says `- kind: code:<lang>`.
fn is_code_page(sections: &BTreeMap<String, String>) -> bool {
    sections.get("Metadata").is_some_and(|m| {
        m.lines()
            .any(|l| l.trim_start().starts_with("- kind: code:"))
    })
}

/// `pages`: page id -> rendered markdown. Purely in-memory (no disk re-read).
pub fn lint(pages: &BTreeMap<String, String>) -> LintReport {
    let known: std::collections::BTreeSet<&String> = pages.keys().collect();
    let mut incoming: BTreeMap<String, usize> = pages.keys().map(|k| (k.clone(), 0)).collect();
    let mut broken_links = Vec::new();
    let mut large = Vec::new();

    for (id, text) in pages {
        let sections = parse_sections(text);
        // A pack separator is charged to every block, so a page over the
        // budget with it is degraded to a summary; code pages are exempt
        // (their outline is the intended view).
        let tokens = (text.chars().count() + Wiki::SEPARATOR.chars().count()) / 4;
        if !is_code_page(&sections) && tokens > crate::serve::DEFAULT_NEIGHBORS_MAX_TOKENS {
            large.push((id.clone(), tokens));
        }
        // Scan a code mask, not the raw page: a `[[slug|Name]]` written as a
        // syntax example in a fenced block or inline code is not a link. A code
        // page goes further: its `## Body` is verbatim source, where a
        // `[[...]]` is a string literal or a comment, so the whole section is
        // dropped before the scan. Text and markdown bodies are scanned — a
        // wikilink there is the author's, and a broken one is the point.
        let scan: Cow<str> = match sections.get("Body") {
            Some(body) if is_code_page(&sections) && !body.is_empty() => {
                text.replacen(body.as_str(), "", 1).into()
            }
            _ => text.into(),
        };
        for cap in LINK.captures_iter(&mask_code(&scan)) {
            let inner = &cap[1];
            if !known.contains(&slugify(link_target(inner))) {
                broken_links.push((id.clone(), link_display(inner).to_string()));
            }
        }
        // Orphan counting uses ONLY the Related section (true outgoing edges).
        if let Some(related) = sections.get("Related") {
            for cap in LINK.captures_iter(related) {
                let slug = slugify(link_target(&cap[1]));
                if let Some(c) = incoming.get_mut(&slug) {
                    *c += 1;
                }
            }
        }
    }

    let mut orphans: Vec<String> = incoming
        .into_iter()
        .filter(|(_, c)| *c == 0)
        .map(|(id, _)| id)
        .collect();
    orphans.sort();
    LintReport {
        total_pages: pages.len(),
        broken_links,
        orphans,
        large,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::model::{Entity, SourceKind};
    use crate::rewrite::render_page;
    use std::collections::BTreeMap;

    fn ent(id: &str, name: &str, body: &str) -> Entity {
        Entity {
            id: id.into(),
            name: name.into(),
            aliases: vec![],
            tags: vec![],
            created: String::new(),
            updated: String::new(),
            memory: false,
            body: body.into(),
            source_path: format!("{id}.txt"),
            kind: SourceKind::Text,
            content_hash: [0u8; 32],
            summary: None,
            symbols: vec![],
            imports: vec![],
            defined: vec![],
            methods: vec![],
            items: vec![],
        }
    }

    fn render_all(v: Vec<Entity>) -> BTreeMap<String, String> {
        let ents: BTreeMap<String, Entity> = v.into_iter().map(|e| (e.id.clone(), e)).collect();
        let g = build_graph(&ents);
        ents.iter()
            .map(|(id, e)| (id.clone(), render_page(e, &g.edges[id], &ents, "")))
            .collect()
    }

    #[test]
    fn does_not_miscount_referenced_by() {
        let pages = render_all(vec![
            ent("alpha", "Alpha", "mentions Beta"),
            ent("beta", "Beta", "nothing"),
        ]);
        let r = lint(&pages);
        assert!(r.orphans.contains(&"alpha".to_string())); // nothing links to Alpha
        assert!(!r.orphans.contains(&"beta".to_string())); // Beta is referenced by Alpha
    }

    #[test]
    fn code_page_body_is_not_scanned_for_links() {
        // The same `[[...]]` literal: in a code body it is source text (a
        // string in a test, a format template), in a text body it is a link.
        let mut code = ent(
            "alpha",
            "Alpha",
            "assert!(s.contains(\"[[ghost|Ghost]]\"));",
        );
        code.kind = SourceKind::Code {
            lang: "rust".into(),
        };
        let text = ent("beta", "Beta", "see [[ghost|Ghost]]");
        let r = lint(&render_all(vec![code, text]));
        assert_eq!(
            r.broken_links,
            vec![("beta".to_string(), "Ghost".to_string())]
        );
    }

    #[test]
    fn broken_link_detected() {
        let mut pages = render_all(vec![ent("alpha", "Alpha", "nothing")]);
        pages
            .get_mut("alpha")
            .unwrap()
            .push_str("\nSee [[Ghost Page]] too.\n");
        let r = lint(&pages);
        assert_eq!(
            r.broken_links,
            vec![("alpha".to_string(), "Ghost Page".to_string())]
        );
    }

    #[test]
    fn clean_wiki_has_no_broken_links() {
        let pages = render_all(vec![
            ent("alpha", "Alpha", "mentions Beta"),
            ent("beta", "Beta", "nothing"),
        ]);
        assert!(lint(&pages).broken_links.is_empty());
    }

    #[test]
    fn resolves_slug_piped_links_and_reports_readable_broken() {
        let mut pages = render_all(vec![
            ent("alpha", "Alpha", "mentions Beta"),
            ent("beta", "Beta", "nothing"),
        ]);
        // A broken slug-piped link: target `ghost_page` has no page.
        pages
            .get_mut("alpha")
            .unwrap()
            .push_str("\nSee [[ghost_page|Ghost Page]].\n");
        let r = lint(&pages);
        // The real Related link `[[beta|Beta]]` resolves (not reported); only the
        // ghost is broken, and it is reported with its READABLE display name.
        assert_eq!(
            r.broken_links,
            vec![("alpha".to_string(), "Ghost Page".to_string())]
        );
        // beta is referenced by alpha's Related section → not an orphan.
        assert!(!r.orphans.contains(&"beta".to_string()));
    }

    #[test]
    fn wikilink_examples_in_code_are_not_broken_links() {
        let mut pages = render_all(vec![ent("alpha", "Alpha", "nothing")]);
        let alpha = pages.get_mut("alpha").unwrap();
        // Inline code: a doc comment explaining the link syntax.
        alpha.push_str("\nLinks look like `[[target|display]]`, and `[[x]]` is bare.\n");
        // Fenced block: a quoted example of a rendered page.
        alpha.push_str("\n```\n## Related\n- [[ghost_page|Ghost Page]]\n```\n");
        assert_eq!(lint(&pages).broken_links, vec![]);
    }

    #[test]
    fn a_real_broken_link_beside_a_code_example_is_still_reported() {
        let mut pages = render_all(vec![ent("alpha", "Alpha", "nothing")]);
        pages
            .get_mut("alpha")
            .unwrap()
            .push_str("\nSyntax is `[[target|display]]`; see [[Ghost Page]].\n");
        assert_eq!(
            lint(&pages).broken_links,
            vec![("alpha".to_string(), "Ghost Page".to_string())]
        );
    }

    #[test]
    fn load_compiled_pages_skips_index_and_agents() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("alpha.md"), "# Alpha").unwrap();
        std::fs::write(tmp.path().join("index.md"), "# Index").unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "# Agents").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "not markdown").unwrap();

        let pages = load_compiled_pages(tmp.path()).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages["alpha"], "# Alpha");
    }

    /// A page whose `(chars + separator) / 4` tokens equal `tokens`.
    fn page_of_tokens(kind: &str, tokens: usize) -> String {
        let head = format!("# T\n\n## Metadata\n- kind: {kind}\n\n## Body\n");
        let chars = tokens * 4 - Wiki::SEPARATOR.chars().count();
        head.clone() + &"x".repeat(chars - head.chars().count())
    }

    fn one(text: String) -> BTreeMap<String, String> {
        BTreeMap::from([("t".to_string(), text)])
    }

    #[test]
    fn large_reports_a_page_over_the_default_pack_budget() {
        let over = crate::serve::DEFAULT_NEIGHBORS_MAX_TOKENS + 1;
        let r = lint(&one(page_of_tokens("markdown", over)));
        assert_eq!(r.large, vec![("t".to_string(), over)]);
        let at = crate::serve::DEFAULT_NEIGHBORS_MAX_TOKENS;
        assert!(lint(&one(page_of_tokens("markdown", at))).large.is_empty());
    }

    #[test]
    fn large_skips_code_pages_and_reports_text_pages() {
        let over = crate::serve::DEFAULT_NEIGHBORS_MAX_TOKENS + 1;
        assert!(lint(&one(page_of_tokens("code:rust", over)))
            .large
            .is_empty());
        let r = lint(&one(page_of_tokens("text", over)));
        assert_eq!(r.large, vec![("t".to_string(), over)]);
    }
}
