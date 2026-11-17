//! Tests for `rewrite`.
use super::*;
use crate::graph::build_graph;
use crate::model::{Entity, SourceKind};
use std::collections::BTreeMap;

#[test]
fn fenced_example_headings_do_not_overwrite_real_sections() {
    // A page whose body shows an example rendered wiki page — exactly what
    // this project's own README does. Unmasked, the fenced `## Body`
    // would win (BTreeMap insert is last-write-wins) and the real body be lost.
    let page = "## Body\nthe real body\n\n```\n## Body\nexample body\n```\n";
    let sections = parse_sections(page);
    assert_eq!(sections.len(), 1);
    let body = &sections["Body"];
    assert!(body.starts_with("the real body"), "got: {body:?}");
    // Sliced from the original, so the fenced example survives verbatim.
    assert!(body.contains("example body"), "got: {body:?}");
}

#[test]
fn fenced_notes_do_not_masquerade_as_preserved_notes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.md");
    std::fs::write(
        &path,
        "## Body\n```\n## Notes\nexample notes\n```\n\n## Notes\nmy real notes\n",
    )
    .unwrap();
    assert_eq!(read_notes_from(&path), "my real notes");
}

#[test]
fn inline_code_at_line_start_does_not_open_a_fence() {
    assert_eq!(mask_fenced_code("```x``` y\n## H\n"), "```x``` y\n## H\n");
}

#[test]
fn mask_preserves_byte_offsets_through_multibyte_code() {
    // Offsets from the mask index back into the original, so a mask that
    // changed length would panic or slice mid-char.
    let page = "## Body\n`«naïve café»`\n\n```\nbody → example\n```\n\n## Notes\nx\n";
    assert_eq!(mask_code(page).len(), page.len());
    assert_eq!(mask_fenced_code(page).len(), page.len());
    let sections = parse_sections(page);
    assert_eq!(sections["Notes"], "x");
    assert!(sections["Body"].contains("«naïve café»"));
}

#[test]
fn unclosed_backtick_run_is_literal_text() {
    let text = "a `b [[ghost]] c\n";
    assert_eq!(mask_code(text), text);
}

#[test]
fn a_stray_backtick_does_not_re_pair_later_lines() {
    // A code page's body is verbatim source; odd backticks in comments are
    // routine. Each line pairs on its own, so line 1 cannot swallow line 3.
    let text = "// a lone ` tick\n[[real_link]]\n// `[[example]]` shown\n";
    let masked = mask_code(text);
    assert_eq!(masked.len(), text.len());
    assert!(masked.contains("[[real_link]]"), "got: {masked:?}");
    assert!(!masked.contains("[[example]]"), "got: {masked:?}");
}

#[test]
fn tilde_fence_closes_only_on_its_own_char() {
    let text = "~~~\n```\n## Inside\n~~~\n\n## Outside\ny\n";
    let sections = parse_sections(text);
    assert_eq!(sections.keys().collect::<Vec<_>>(), vec!["Outside"]);
}

fn ent(id: &str, name: &str, body: &str) -> Entity {
    Entity {
        id: id.into(),
        name: name.into(),
        created: "2026-01-01".into(),
        body: body.into(),
        source_path: format!("{id}.txt"),
        ..Entity::default()
    }
}
fn map(v: Vec<Entity>) -> BTreeMap<String, Entity> {
    v.into_iter().map(|e| (e.id.clone(), e)).collect()
}

#[test]
fn importers_and_mentions_render_apart() {
    let mut text = ent("text", "Text", "");
    text.source_path = "pkg/text.py".into();
    text.kind = SourceKind::Code {
        lang: "python".into(),
    };
    let mut consumer = ent("consumer", "Consumer", "");
    consumer.source_path = "pkg/consumer.py".into();
    consumer.kind = text.kind.clone();
    consumer.imports = vec![".text".into()];
    let ents = map(vec![text, consumer, ent("notes", "Notes", "see `text.py`")]);
    let g = build_graph(&ents);
    let page = render_page(
        &ents["text"],
        &g.edges["text"],
        &ents,
        &path_index(&ents),
        "",
    );
    assert!(
        page.contains(
            "## Imported by\n- [[consumer|Consumer]]\n\n## Mentioned in\n- [[notes|Notes]]\n"
        ),
        "{page}"
    );
}

#[test]
fn front_matter_lists_kind_tag_then_tags_after_aliases() {
    let mut e = ent("alpha", "Alpha", "body");
    e.aliases = vec!["x".into()];
    e.tags = vec!["a".into(), "b/c".into(), "two words".into()];
    let ents = map(vec![e]);
    let g = build_graph(&ents);
    let page = render_page(
        &ents["alpha"],
        &g.edges["alpha"],
        &ents,
        &path_index(&ents),
        "",
    );
    assert!(
        page.starts_with(
            "---\ncreated: 2026-01-01\naliases: [\"x\"]\n\
                 tags: [\"wiki/doc\",\"a\",\"b/c\",\"two-words\"]\nkind: text\n\
                 source: \"alpha.txt\"\nsource_hash: "
        ),
        "{page}"
    );
    assert!(
        !page.contains("## Metadata") && !page.contains("title:"),
        "{page}"
    );
    assert!(page.contains("\n---\n# Alpha\n"), "{page}");
}

#[test]
fn front_matter_kind_tag_follows_the_page_kind() {
    let tag = |f: &dyn Fn(&mut Entity)| {
        let mut e = ent("alpha", "Alpha", "body");
        f(&mut e);
        let ents = map(vec![e]);
        let g = build_graph(&ents);
        render_page(
            &ents["alpha"],
            &g.edges["alpha"],
            &ents,
            &path_index(&ents),
            "",
        )
    };
    assert!(tag(&|e| e.kind = SourceKind::Code {
        lang: "rust".into()
    })
    .contains("tags: [\"wiki/code\"]"));
    assert!(tag(&|e| e.memory = true).contains("tags: [\"wiki/memory\"]"));
}

#[test]
fn code_body_is_fenced_with_its_language_and_a_long_enough_fence() {
    let mut e = ent("alpha", "Alpha", "fn a() {}\n```\nx\n```");
    e.kind = SourceKind::Code {
        lang: "rust".into(),
    };
    let ents = map(vec![e]);
    let g = build_graph(&ents);
    let page = render_page(
        &ents["alpha"],
        &g.edges["alpha"],
        &ents,
        &path_index(&ents),
        "",
    );
    assert!(
        page.contains("## Body\n````rust\nfn a() {}\n```\nx\n```\n````\n\n## Notes"),
        "{page}"
    );
}

#[test]
fn page_source_index_reads_front_matter_and_old_metadata_alike() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("new.md"),
        "---\nkind: text\nsource: \"dir/a \\\"b\\\".md\"\n---\n# N\n\n## Notes\nhand\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("old.md"),
        "# O\n\n## Metadata\n- source: old.md\n\n## Notes\nhand\n",
    )
    .unwrap();
    let idx = page_source_index(dir.path());
    assert!(idx.contains_key("dir/a \"b\".md"), "{idx:?}");
    assert!(idx.contains_key("old.md"), "{idx:?}");
}

#[test]
fn fingerprint_changes_with_content_hash() {
    let a = ent("alpha", "Alpha", "body");
    let mut b = a.clone();
    b.content_hash = [1u8; 32];
    let ents = map(vec![a.clone()]);
    let g = build_graph(&ents);
    let e = &g.edges["alpha"];
    assert_ne!(
        render_fingerprint(&a, e, &ents, ""),
        render_fingerprint(&b, e, &ents, "")
    );
}

#[test]
fn sanitize_display_replaces_pipe() {
    assert_eq!(sanitize_display("A|B"), "A/B");
    assert_eq!(sanitize_display("Normal Name"), "Normal Name");
}

#[test]
fn preserved_notes_survive() {
    let ents = map(vec![ent("alpha", "Alpha", "body")]);
    let g = build_graph(&ents);
    let page = render_page(
        &ents["alpha"],
        &g.edges["alpha"],
        &ents,
        &path_index(&ents),
        "MY HAND NOTES",
    );
    assert!(page.contains("MY HAND NOTES"));
    let sections = parse_sections(&page);
    assert_eq!(
        sections.get("Notes").map(|s| s.trim()),
        Some("MY HAND NOTES")
    );
}

#[test]
fn body_and_notes_keep_their_own_heading_lines() {
    // A code body may hold `## ` lines (a heredoc, a doc string) and hand
    // notes may have subheadings: neither is a page section.
    let mut e = ent(
        "alpha",
        "Alpha",
        "x\n## Metadata\n- source: fake\n## Notes\ny",
    );
    e.kind = SourceKind::Code {
        lang: "bash".into(),
    };
    let ents = map(vec![e]);
    let g = build_graph(&ents);
    let page = render_page(
        &ents["alpha"],
        &g.edges["alpha"],
        &ents,
        &path_index(&ents),
        "mine\n## Todo\nmore",
    );
    let sections = parse_sections(&page);
    assert_eq!(
        sections["Body"],
        "```bash\nx\n## Metadata\n- source: fake\n## Notes\ny\n```"
    );
    assert_eq!(sections["Notes"], "mine\n## Todo\nmore");
    assert_eq!(sections.keys().collect::<Vec<_>>(), vec!["Body", "Notes"]);
}

#[test]
fn fingerprint_changes_with_incoming_edges() {
    let ents1 = map(vec![
        ent("a", "Alpha", "nothing"),
        ent("b", "Beta", "nothing"),
    ]);
    let g1 = build_graph(&ents1);
    let f1 = render_fingerprint(&ents1["a"], &g1.edges["a"], &ents1, "");
    let ents2 = map(vec![
        ent("a", "Alpha", "nothing"),
        ent("b", "Beta", "mentions Alpha"),
    ]);
    let g2 = build_graph(&ents2);
    let f2 = render_fingerprint(&ents2["a"], &g2.edges["a"], &ents2, "");
    assert_ne!(f1, f2); // Alpha gained an incoming edge → must re-render
}

#[test]
fn page_source_index_reads_metadata_and_ignores_fenced_examples() {
    let dir = tempfile::tempdir().unwrap();
    let real = "# Order Book\n\n## Metadata\n- source: src/orderbook.h\n\n## Body\nreal page\n";
    std::fs::write(dir.path().join("order_book.md"), real).unwrap();

    // A doc page whose BODY quotes an example page. The quoted `- source:`
    // must not register: it would steal notes from the real page later.
    let doc = "# Readme\n\n## Metadata\n- source: README.md\n\n## Body\n```\n## Metadata\n- source: src/orderbook.h\n```\n";
    std::fs::write(dir.path().join("readme.md"), doc).unwrap();

    // Manifests are not pages and must be skipped.
    std::fs::write(
        dir.path().join("index.md"),
        "## Metadata\n- source: nope.txt\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "## Metadata\n- source: nope2.txt\n",
    )
    .unwrap();

    let idx = page_source_index(dir.path());
    let names = |src: &str| -> Vec<String> {
        idx.get(src)
            .into_iter()
            .flatten()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect()
    };

    assert_eq!(
        names("src/orderbook.h"),
        vec!["order_book.md".to_string()],
        "fenced example stole the mapping: {idx:?}"
    );
    assert_eq!(names("README.md"), vec!["readme.md".to_string()]);
    assert!(
        !idx.contains_key("nope.txt"),
        "index.md is a manifest, not a page"
    );
    assert!(
        !idx.contains_key("nope2.txt"),
        "AGENTS.md is a manifest, not a page"
    );
}

#[test]
fn page_source_index_prefers_front_matter_over_metadata() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("p.md"),
        "---\nsource: \"a.rs\"\n---\n# P\n\n## Metadata\n- source: b.rs\n",
    )
    .unwrap();

    let idx = page_source_index(dir.path());
    assert_eq!(idx.len(), 1, "{idx:?}");
    assert_eq!(idx["a.rs"], BTreeSet::from([dir.path().join("p.md")]));
}

#[test]
fn page_source_index_keeps_every_front_matter_page_declaring_one_source() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["one.md", "two.md"] {
        std::fs::write(dir.path().join(name), "---\nsource: \"a.rs\"\n---\n# A\n").unwrap();
    }

    let idx = page_source_index(dir.path());
    assert_eq!(idx.len(), 1, "{idx:?}");
    assert_eq!(
        idx["a.rs"],
        BTreeSet::from([dir.path().join("one.md"), dir.path().join("two.md")])
    );
}

#[test]
fn page_source_index_skips_unreadable_pages() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bad.md"), [0xffu8, 0xfe]).unwrap();
    std::fs::write(
        dir.path().join("good.md"),
        "---\nsource: \"a.rs\"\n---\n# G\n",
    )
    .unwrap();

    let idx = page_source_index(dir.path());
    assert_eq!(idx.len(), 1, "{idx:?}");
    assert_eq!(idx["a.rs"], BTreeSet::from([dir.path().join("good.md")]));
}

#[test]
fn prior_page_for_prefers_the_page_s_own_file_over_a_smaller_orphan() {
    let orphan = PathBuf::from("/out/alpha_one.md");
    let own = PathBuf::from("/out/alpha_two.md");
    let both: BTreeSet<PathBuf> = [orphan.clone(), own.clone()].into_iter().collect();

    // Own file present: it wins, even though the orphan sorts first.
    assert_eq!(prior_page_for(&both, &own), Some(own.clone()));
    // Own file absent — a genuine rename: the smallest candidate wins,
    // deterministically.
    let renamed = PathBuf::from("/out/alpha_three.md");
    assert_eq!(prior_page_for(&both, &renamed), Some(orphan));
    // Nothing declares this source: the caller falls back to its own file.
    assert_eq!(prior_page_for(&BTreeSet::new(), &own), None);
}

#[test]
fn front_matter_shows_updated_when_known() {
    let mut e = ent("alpha", "Alpha", "x");
    let render = |e: &Entity| {
        let ents = map(vec![e.clone()]);
        render_page(
            e,
            &build_graph(&ents).edges["alpha"],
            &ents,
            &path_index(&ents),
            "",
        )
    };
    assert!(!render(&e).contains("updated:"));
    e.updated = "2026-10-23".into();
    assert!(render(&e).contains("created: 2026-01-01\nupdated: 2026-10-23\n"));
    e.created.clear();
    assert!(!render(&e).contains("created:"));
}

fn fix(text: &str) -> String {
    let ids = ["alpha", "wiki_rig"];
    obsidian_fixups(text, "docs/a.md", &|id| ids.contains(&id), &|p| match p {
        "README.md" => Some("proj".to_string()),
        "docs/b.md" => Some("beta".to_string()),
        _ => None,
    })
}

#[test]
fn wikilink_to_a_slug_resolved_page_is_written_as_its_id() {
    assert_eq!(
        fix("see [[wiki-rig]] and [[Wiki Rig|rig]]"),
        "see [[wiki_rig]] and [[wiki_rig|rig]]"
    );
    assert_eq!(fix("[[wiki-rig#Part]]"), "[[wiki_rig#Part]]");
    assert_eq!(fix("[[alpha]] [[nope-x]]"), "[[alpha]] [[nope-x]]");
}

#[test]
fn relative_markdown_links_to_pages_become_page_files() {
    assert_eq!(
        fix("[r](../README.md) [b](b.md#x) [l](../LICENSE) [e](https://x.io/a.md)"),
        "[r](proj.md) [b](beta.md#x) [l](../LICENSE) [e](https://x.io/a.md)"
    );
}

#[test]
fn hash_is_escaped_only_where_obsidian_reads_a_tag() {
    assert_eq!(
        fix("#76\u{2192} x #1\u{2013}#9 #43/ (#12-) PR #76"),
        "\\#76\u{2192} x #1\u{2013}#9 #43/ (#12-) PR #76"
    );
    assert_eq!(
        fix("a #37-x #57-59 #12 #abc"),
        "a \\#37-x \\#57-59 #12 #abc"
    );
}

#[test]
fn fixups_skip_fenced_and_inline_code() {
    let t = "`[[wiki-rig]] #76-x [r](../README.md)`\n```\n[[wiki-rig]] #76-x\n```\n";
    assert_eq!(fix(t), t);
}

#[test]
fn render_page_applies_fixups_to_text_bodies_only() {
    let mut a = ent("alpha", "Alpha", "[[wiki-rig]] [t](docs/beta.txt) #5-x");
    a.source_path = "a.txt".into();
    let mut b = ent("wiki_rig", "Rig", "x");
    b.source_path = "docs/beta.txt".into();
    let mut c = ent("code", "Code", "// [[wiki-rig]]");
    c.kind = SourceKind::Code {
        lang: "rust".into(),
    };
    let ents = map(vec![a, b, c]);
    let g = build_graph(&ents);
    let page = render_page(
        &ents["alpha"],
        &g.edges["alpha"],
        &ents,
        &path_index(&ents),
        "",
    );
    assert!(
        page.contains("## Body\n[[wiki_rig]] [t](wiki_rig.md) \\#5-x\n"),
        "{page}"
    );
    let page = render_page(
        &ents["code"],
        &g.edges["code"],
        &ents,
        &path_index(&ents),
        "",
    );
    assert!(page.contains("// [[wiki-rig]]"), "{page}");
}

/// A link left at the temporary path (a dead run, or planted) must not make the
/// write land in the file it points at.
#[cfg(unix)] // unix: needs a symlink
#[test]
fn write_atomic_does_not_write_through_a_link_at_the_temporary_path() {
    let out = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim.txt");
    std::fs::write(&victim, "keep me").unwrap();
    let target = out.path().join("index.md");
    let tmp = out.path().join("index.md.tmp");
    std::os::unix::fs::symlink(&victim, &tmp).unwrap();

    write_atomic(&target, "new content").unwrap();

    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new content");
    assert!(
        std::fs::symlink_metadata(&tmp).is_err(),
        "the temporary path is gone"
    );
}
