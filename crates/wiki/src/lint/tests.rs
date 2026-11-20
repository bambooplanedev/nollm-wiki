//! Tests for `lint`.
use super::*;
use crate::graph::build_graph;
use crate::model::{Entity, SourceKind};
use crate::rewrite::{path_index, render_page};
use std::collections::BTreeMap;

fn ent(id: &str, name: &str, body: &str) -> Entity {
    Entity {
        id: id.into(),
        name: name.into(),
        body: body.into(),
        source_path: format!("{id}.txt"),
        ..Entity::default()
    }
}

fn render_all(v: Vec<Entity>) -> BTreeMap<String, String> {
    let ents: BTreeMap<String, Entity> = v.into_iter().map(|e| (e.id.clone(), e)).collect();
    let g = build_graph(&ents);
    let bp = path_index(&ents);
    ents.iter()
        .map(|(id, e)| (id.clone(), render_page(e, &g.edges[id], &ents, &bp, "")))
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
fn a_heading_line_in_a_code_body_does_not_end_the_body() {
    // A heredoc writing markdown: its `## ` line is source text, not a
    // section, so the rest of the body stays unscanned code.
    let mut code = ent(
        "alpha",
        "Alpha",
        "cat <<EOF\n## Usage\nsee [[ghost|Ghost]]\nEOF",
    );
    code.kind = SourceKind::Code {
        lang: "bash".into(),
    };
    assert_eq!(lint(&render_all(vec![code])).broken_links, vec![]);
}

#[test]
fn a_markdown_body_with_its_own_headings_is_still_scanned() {
    let doc = ent("beta", "Beta", "## Usage\nsee [[ghost|Ghost]]");
    assert_eq!(
        lint(&render_all(vec![doc])).broken_links,
        vec![("beta".to_string(), "Ghost".to_string())]
    );
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
    sized(&format!("---\nkind: {kind}\n---\n# T\n\n## Body\n"), tokens)
}

/// The same, in the `## Metadata` form of pages compiled by 0.6.7 or earlier.
fn old_page_of_tokens(kind: &str, tokens: usize) -> String {
    sized(
        &format!("# T\n\n## Metadata\n- kind: {kind}\n\n## Body\n"),
        tokens,
    )
}

fn sized(head: &str, tokens: usize) -> String {
    let chars = tokens * 4 - Wiki::SEPARATOR.chars().count();
    head.to_string() + &"x".repeat(chars - head.chars().count())
}

fn one(text: String) -> BTreeMap<String, String> {
    BTreeMap::from([("t".to_string(), text)])
}

#[test]
fn large_reports_a_page_over_the_default_pack_budget() {
    let over = crate::query::DEFAULT_NEIGHBORS_MAX_TOKENS + 1;
    let r = lint(&one(page_of_tokens("markdown", over)));
    assert_eq!(r.large, vec![("t".to_string(), over)]);
    let at = crate::query::DEFAULT_NEIGHBORS_MAX_TOKENS;
    assert_eq!(
        lint(&one(page_of_tokens("markdown", at))).large,
        [] as [(String, usize); 0]
    );
}

#[test]
fn large_skips_old_form_code_pages() {
    let over = crate::query::DEFAULT_NEIGHBORS_MAX_TOKENS + 1;
    assert_eq!(
        lint(&one(old_page_of_tokens("code:rust", over))).large,
        [] as [(String, usize); 0]
    );
    assert_eq!(lint(&one(old_page_of_tokens("text", over))).large.len(), 1);
}

#[test]
fn large_skips_code_pages_and_reports_text_pages() {
    let over = crate::query::DEFAULT_NEIGHBORS_MAX_TOKENS + 1;
    assert_eq!(
        lint(&one(page_of_tokens("code:rust", over))).large,
        [] as [(String, usize); 0]
    );
    let r = lint(&one(page_of_tokens("text", over)));
    assert_eq!(r.large, vec![("t".to_string(), over)]);
}
