use crate::model::{slugify, LintReport};
use crate::rewrite::parse_sections;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[(.+?)\]\]").unwrap());

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

/// `pages`: page id -> rendered markdown. Purely in-memory (no disk re-read).
pub fn lint(pages: &BTreeMap<String, String>) -> LintReport {
    let known: std::collections::BTreeSet<&String> = pages.keys().collect();
    let mut incoming: BTreeMap<String, usize> = pages.keys().map(|k| (k.clone(), 0)).collect();
    let mut broken_links = Vec::new();

    for (id, text) in pages {
        for cap in LINK.captures_iter(text) {
            let inner = &cap[1];
            if !known.contains(&slugify(link_target(inner))) {
                broken_links.push((id.clone(), link_display(inner).to_string()));
            }
        }
        // Orphan counting uses ONLY the Related section (true outgoing edges).
        if let Some(related) = parse_sections(text).get("Related") {
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
            created: String::new(),
            body: body.into(),
            source_path: format!("{id}.txt"),
            kind: SourceKind::Text,
            content_hash: [0u8; 32],
            summary: None,
            symbols: vec![],
            imports: vec![],
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
}
