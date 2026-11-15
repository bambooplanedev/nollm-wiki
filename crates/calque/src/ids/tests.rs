//! Tests for page-id disambiguation, reserved-name remapping and free-id assignment.
use super::*;

fn ent(id: &str) -> Entity {
    Entity {
        id: id.into(),
        name: id.into(),
        body: String::new(),
        source_path: format!("{id}.txt"),
        ..Entity::default()
    }
}

fn map(ids: &[&str]) -> BTreeMap<String, Entity> {
    ids.iter().map(|id| (id.to_string(), ent(id))).collect()
}

fn ent_at(name: &str, path: &str) -> Entity {
    let mut e = ent(name);
    e.id = crate::model::slugify(name);
    e.name = name.into();
    e.source_path = path.into();
    e
}

fn ids_after_disambiguation(pairs: &[(&str, &str)]) -> Vec<String> {
    let mut v: Vec<Entity> = pairs.iter().map(|(n, p)| ent_at(n, p)).collect();
    disambiguate_ids(&mut v);
    v.into_iter().map(|e| e.id).collect()
}

fn code_entity(path: &str) -> Entity {
    let mut e = ent(path);
    e.source_path = path.to_string();
    e.kind = crate::model::SourceKind::Code { lang: "cpp".into() };
    e
}

fn text_entity(path: &str) -> Entity {
    let mut e = ent(path);
    e.source_path = path.to_string();
    e.kind = crate::model::SourceKind::Markdown;
    e
}

#[test]
fn discriminator_is_the_tail_for_code_and_the_stem_for_prose() {
    // No infix between stem and final extension: the extension itself.
    assert_eq!(discriminator(&code_entity("src/orderbook.h")), "h");
    assert_eq!(discriminator(&code_entity("src/orderbook.cpp")), "cpp");
    assert_eq!(discriminator(&code_entity("api/client.ts")), "ts");
    // An infix: the segments between stem and final extension, joined by
    // `_` — NOT the full dotted tail ("test.ts"/"test.spec.ts"), which
    // would put the final extension into the rendered name as its own
    // word ("Client Test Ts") — a token that appears in no other page's
    // body.
    assert_eq!(discriminator(&code_entity("api/client.test.ts")), "test");
    assert_eq!(
        discriminator(&code_entity("api/foo.test.spec.ts")),
        "test_spec"
    );
    // Directory module: module_stem returns "pkg", not the basename, so the
    // tail is not what distinguishes it.
    assert_eq!(discriminator(&code_entity("pkg/__init__.py")), "init");
    // Prose: the name came from the document title, so the stem distinguishes.
    assert_eq!(discriminator(&text_entity("docs/v1.md")), "v1");
    assert_eq!(discriminator(&text_entity("docs/v2.md")), "v2");
}

/// Run the pass and return (id, name) pairs in input order.
fn ids_of(mut ents: Vec<Entity>) -> Vec<(String, String)> {
    for e in &mut ents {
        e.id = crate::model::slugify(&e.name);
    }
    disambiguate_ids(&mut ents);
    ents.into_iter().map(|e| (e.id, e.name)).collect()
}

fn named_code(path: &str, name: &str) -> Entity {
    let mut e = code_entity(path);
    e.name = name.to_string();
    e
}

#[test]
fn header_keeps_the_short_name_and_impl_takes_the_discriminator() {
    let got = ids_of(vec![
        named_code("src/orderbook.cpp", "Orderbook"),
        named_code("src/orderbook.h", "Orderbook"),
    ]);
    assert_eq!(got[1].0, "orderbook", "the .h must keep the bare slug");
    assert_eq!(got[0].0, "orderbook_cpp");
}

#[test]
fn three_way_header_group_keeps_a_strict_winner() {
    let got = ids_of(vec![
        named_code("src/ob.h", "Ob"),
        named_code("src/ob.hpp", "Ob"),
        named_code("src/ob.cpp", "Ob"),
    ]);
    assert_eq!(got[0].0, "ob", ".h alone is rank 0, so it wins strictly");
    assert_eq!(got[1].0, "ob_hpp");
    assert_eq!(got[2].0, "ob_cpp");
}

#[test]
fn a_tie_exempts_nobody() {
    let got = ids_of(vec![
        named_code("src/utils.js", "Utils"),
        named_code("src/utils.ts", "Utils"),
    ]);
    assert_eq!(got[0].0, "utils_js");
    assert_eq!(got[1].0, "utils_ts");
    assert!(
        !got.iter().any(|(id, _)| id == "utils"),
        "a build artifact must not take the canonical name: {got:?}"
    );
}

#[test]
fn test_sibling_loses_to_the_plain_source() {
    let got = ids_of(vec![
        named_code("api/client.test.ts", "Client"),
        named_code("api/client.ts", "Client"),
    ]);
    assert_eq!(got[1].0, "client");
    assert_eq!(got[0].0, "client_test");
}

#[test]
fn directory_module_and_sibling_both_survive() {
    let got = ids_of(vec![
        named_code("src/foo.rs", "Foo"),
        named_code("src/foo/mod.rs", "Foo"),
    ]);
    let ids: Vec<_> = got.iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(
        ids[0], ids[1],
        "src/foo.rs and src/foo/mod.rs both need a page: {got:?}"
    );
}

#[test]
fn ordinals_rescue_a_group_the_discriminator_cannot_split() {
    // Both basenames slugify to the same stem AND the same tail.
    let got = ids_of(vec![
        named_code("a/foo-bar.py", "Foo Bar"),
        named_code("a/foo_bar.py", "Foo Bar"),
    ]);
    assert_ne!(
        got[0].0, got[1].0,
        "ordinal rung must separate them: {got:?}"
    );
}

#[test]
fn case_variants_get_two_pages_not_one() {
    let got = ids_of(vec![
        named_code("src/Book.h", "Book"),
        named_code("src/book.h", "Book"),
    ]);
    assert_ne!(
        got[0].0, got[1].0,
        "two distinct files need two pages: {got:?}"
    );
}

#[test]
fn redundant_discriminator_is_skipped() {
    let mut prose = text_entity("a/orderbook.md");
    prose.name = "Orderbook".into();
    let got = ids_of(vec![named_code("a/orderbook.h", "Orderbook"), prose]);
    assert!(
        !got.iter().any(|(_, n)| n.contains("Orderbook Orderbook")),
        "doubled token makes the page unreachable by mention: {got:?}"
    );
    assert_ne!(got[0].0, got[1].0);
}

#[test]
fn every_id_is_the_slug_of_its_name() {
    let got = ids_of(vec![
        named_code("a/foo-bar.py", "Foo Bar"),
        named_code("a/foo_bar.py", "Foo Bar"),
        named_code("src/ob.h", "Ob"),
        named_code("src/ob.cpp", "Ob"),
    ]);
    for (id, name) in &got {
        assert_eq!(*id, crate::model::slugify(name), "id/name drift on {name}");
    }
}

#[test]
fn result_does_not_depend_on_input_order() {
    let make = || {
        vec![
            named_code("a/foo.h", "Foo"),
            named_code("a/foo.cpp", "Foo"),
            named_code("b/foo.h", "Foo"),
            named_code("b/foo.cpp", "Foo"),
            named_code("a/foo-x.py", "Foo X"),
            named_code("a/foo_x.py", "Foo X"),
        ]
    };
    let mut baseline: Vec<(String, String)> = ids_of(make())
        .into_iter()
        .zip(make())
        .map(|((id, _), e)| (e.source_path, id))
        .collect();
    baseline.sort();

    // Every rotation of the input must produce the same source_path -> id map.
    for rot in 1..6 {
        let mut ents = make();
        ents.rotate_left(rot);
        let paths: Vec<String> = ents.iter().map(|e| e.source_path.clone()).collect();
        let mut got: Vec<(String, String)> = ids_of(ents)
            .into_iter()
            .zip(paths)
            .map(|((id, _), p)| (p, id))
            .collect();
        got.sort();
        assert_eq!(got, baseline, "rotation {rot} changed the result");
    }
}

fn named_text(path: &str, name: &str) -> Entity {
    let mut e = text_entity(path);
    e.name = name.to_string();
    e
}

#[test]
fn four_way_title_collision_all_survive() {
    // docs/2.md's OWN title already renders the ordinal-shaped id
    // "chapter_2" — a coincidence the free-id search must discover by
    // checking every current id, not by counting positions within the
    // {Chapter.md, chapter.md} group. Assigning by position would give
    // chapter.md "chapter_2", colliding with docs/2.md and losing a page
    // to the first-wins dedup.
    let got = ids_of(vec![
        named_text("docs/1.md", "Chapter 1"),
        named_text("docs/2.md", "Chapter 2"),
        named_text("docs/Chapter.md", "chapter"),
        named_text("docs/chapter.md", "Chapter"),
    ]);
    let ids: Vec<&str> = got.iter().map(|(id, _)| id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        4,
        "all four files must get distinct pages: {got:?}"
    );
}

#[test]
fn free_id_search_clears_a_stale_ordinal_at_position_zero() {
    // Simulate a group entering the free-id search with position 0
    // (a/foo.h, first by source_path) carrying an ordinal left over from
    // an earlier round — e.g. it once shared this same id string with a
    // blocker that has since moved away. Nothing blocks the bare name
    // now, so the search must clear it rather than leave it.
    let extracted = vec![named_code("a/foo.h", "Foo"), named_code("a/foo.hpp", "Foo")];
    let base: Vec<String> = extracted.iter().map(|e| e.name.clone()).collect();
    let disc = vec![String::new(), String::new()];
    let depth = vec![0usize, 0usize];
    let used_disc = vec![false, false];
    let mut ordinal = vec![2usize, 0usize]; // stale: position 0 already at 2
    let mut claimed: BTreeSet<String> = extracted.iter().map(|e| e.id.clone()).collect();

    let state = LadderState {
        base: &base,
        disc: &disc,
        depth: &depth,
        used_disc: &used_disc,
    };
    let changed = assign_free_ids(
        &[0, 1],
        &extracted,
        &state,
        &mut ordinal,
        &mut claimed,
        &BTreeSet::new(),
    );

    assert!(changed);
    assert_eq!(
        ordinal[0], 0,
        "position 0's stale ordinal must be cleared once nothing blocks the bare name"
    );
    assert_eq!(ordinal[1], 2);
}

#[test]
fn discriminator_redundancy_checks_every_word_the_discriminator_carries() {
    // A single-word discriminator: unchanged behavior.
    assert!(discriminator_is_redundant("Orderbook", "orderbook"));
    assert!(!discriminator_is_redundant("Orderbook", "cpp"));
    // A multi-word discriminator: comparing only the LAST token would
    // wrongly call this non-redundant ("spec" != "test spec").
    assert!(discriminator_is_redundant("Foo Test Spec", "test_spec"));
    assert!(!discriminator_is_redundant("Foo Bar", "test_spec"));
    assert!(!discriminator_is_redundant("Foo Spec", "test_spec"));
}

#[test]
fn multi_word_redundant_discriminator_is_skipped() {
    // foo.test.spec.ts's discriminator (the infix rule) is
    // "test_spec". Its own name already ends in "Test Spec", so applying
    // the discriminator would render "Foo Test Spec Test Spec" — a
    // doubled tail unreachable by mention, exactly what the guard
    // exists to prevent.
    let mut other = text_entity("a/other.md");
    other.name = "Foo Test Spec".into();
    let got = ids_of(vec![
        named_code("a/foo.test.spec.ts", "Foo Test Spec"),
        other,
    ]);
    assert!(
        !got.iter().any(|(_, n)| n.contains("Test Spec Test Spec")),
        "doubled multi-word tail makes the page unreachable by mention: {got:?}"
    );
    assert_ne!(got[0].0, got[1].0);
}

#[test]
fn colliding_basenames_are_qualified_by_their_parent_directory() {
    // The canonical Python app layout: every package has its own
    // `models.py`, and before qualification all but the first were
    // dropped outright by the dedup below.
    assert_eq!(
        ids_after_disambiguation(&[
            ("Models", "app/api/models.py"),
            ("Models", "app/db/models.py"),
            ("Models", "app/web/models.py"),
        ]),
        vec!["api_models", "db_models", "web_models"]
    );
}

#[test]
fn a_unique_basename_is_left_alone() {
    // Qualification is collision-driven: a page that never collides keeps
    // the short title, so a rename is always evidence of a real clash.
    assert_eq!(
        ids_after_disambiguation(&[
            ("Graph", "src/graph.rs"),
            ("Models", "app/api/models.py"),
            ("Store", "app/db/store.py"),
        ]),
        vec!["graph", "models", "store"]
    );
}

#[test]
fn qualification_extends_until_unique() {
    // One directory is not enough here — both files sit under a `mod`
    // directory named `x`, so the walk has to take a second segment.
    assert_eq!(
        ids_after_disambiguation(&[("Mod", "a/x/mod.rs"), ("Mod", "b/x/mod.rs")]),
        vec!["a_x_mod", "b_x_mod"]
    );
}

#[test]
fn a_root_file_that_cannot_extend_keeps_its_name() {
    // A file at the project root has no segment left to prefix. It keeps
    // the bare id and the file that *can* extend moves aside, so the pair
    // still separates without either page being dropped.
    assert_eq!(
        ids_after_disambiguation(&[("Readme", "README.md"), ("Readme", "docs/README.md")]),
        vec!["readme", "docs_readme"]
    );
}

#[test]
fn qualification_preserves_a_content_derived_title() {
    // Markdown and text pages take their name from an H1 or frontmatter
    // title, not from the path. Qualification must PREFIX that title, not
    // replace it with the filename.
    let mut v = vec![
        ent_at("Overview", "docs/api/index.md"),
        ent_at("Overview", "docs/db/index.md"),
    ];
    disambiguate_ids(&mut v);
    assert_eq!(v[0].name, "Api Overview");
    assert_eq!(v[1].name, "Db Overview");
}

#[test]
fn reserved_id_is_remapped_and_entity_id_field_matches_key() {
    let out = remap_reserved_names(map(&["index", "alpha"]));
    assert!(!out.contains_key("index"));
    assert!(out.contains_key("index_page"));
    assert_eq!(out["index_page"].id, "index_page");
    assert!(out.contains_key("alpha"));
}

#[test]
fn all_reserved_names_are_remapped() {
    let out = remap_reserved_names(map(&["index", "llms", "agents", "graph"]));
    for reserved in ["index", "llms", "agents", "graph"] {
        assert!(!out.contains_key(reserved));
        assert!(out.contains_key(&format!("{reserved}_page")));
    }
}

#[test]
fn remap_avoids_colliding_with_an_existing_fixed_id() {
    // "index_page" is already taken by a real (non-reserved) entity, so
    // "index" must skip straight past it to "index_page_2".
    let out = remap_reserved_names(map(&["index", "index_page"]));
    assert!(!out.contains_key("index"));
    assert!(out.contains_key("index_page_2"));
    assert_eq!(out["index_page_2"].id, "index_page_2");
    // The pre-existing entity keeps its original id untouched.
    assert_eq!(out["index_page"].id, "index_page");
}

#[test]
fn remap_is_deterministic_regardless_of_input_order() {
    let a = remap_reserved_names(map(&["index", "agents", "alpha"]));
    let b = remap_reserved_names(map(&["alpha", "agents", "index"]));
    let ids_a: Vec<&String> = a.keys().collect();
    let ids_b: Vec<&String> = b.keys().collect();
    assert_eq!(ids_a, ids_b);
}
