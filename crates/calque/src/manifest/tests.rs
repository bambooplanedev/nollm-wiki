//! Tests for `manifest`.
use super::*;
use crate::graph::build_graph;
use crate::model::{CodeItemKind, PairRole};

fn ent(id: &str, name: &str, body: &str, summary: Option<&str>) -> Entity {
    Entity {
        id: id.into(),
        name: name.into(),
        body: body.into(),
        source_path: format!("{id}.txt"),
        summary: summary.map(String::from),
        ..Entity::default()
    }
}

fn setup() -> (BTreeMap<String, Entity>, Graph) {
    let ents: BTreeMap<String, Entity> = vec![
        ent("alpha", "Alpha", "mentions Beta", Some("Alpha summary.")),
        ent("beta", "Beta", "nothing", None),
    ]
    .into_iter()
    .map(|e| (e.id.clone(), e))
    .collect();
    let g = build_graph(&ents);
    (ents, g)
}

#[test]
fn manifest_has_sorted_entries_with_degrees() {
    let (ents, g) = setup();
    let m = build_manifest("proj", "", &ents, &g);
    assert_eq!(m.entries[0].id, "alpha");
    assert_eq!(m.entries[0].degree_out, 1);
    assert_eq!(m.entries[1].degree_in, 1); // beta referenced by alpha
}

#[test]
fn index_json_is_valid_json_and_llms_txt_has_h1() {
    let (ents, g) = setup();
    let m = build_manifest("My Project", "", &ents, &g);
    let json = render_index_json(&m);
    let _v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(render_llms_txt(&m).starts_with("# My Project"));
    let agents = render_agents_md("My Project");
    assert!(agents.contains("index.json"));
    assert!(agents.contains("items.json") && agents.contains("calque page"));
}

#[test]
fn a_page_without_a_summary_ends_its_index_lines_cleanly() {
    let (ents, g) = setup();
    let m = build_manifest("proj", "", &ents, &g);
    let llms = render_llms_txt(&m);
    let md = render_index_md(&m);
    assert!(
        llms.contains("- [Alpha](alpha.md): Alpha summary.\n"),
        "{llms}"
    );
    assert!(llms.contains("- [Beta](beta.md)\n"), "{llms}");
    assert!(md.contains("(in 1, out 0)\n"), "{md}");
    assert!(!llms.contains(": \n") && !md.contains(": \n"));
}

#[test]
fn index_lines_carry_one_clipped_summary_line() {
    let uk = "Слово ".repeat(40); // 240 chars, Cyrillic
    let uk_cut = format!("{}…", uk.trim().chars().take(159).collect::<String>());
    let wrapped = "first line\nsecond   line\n\nthird";
    let exact = "x".repeat(LINE_CHARS); // exactly 160 chars
    let ents: BTreeMap<String, Entity> = vec![
        ent("uk", "Uk", "nothing", Some(&uk)),
        ent("nl", "Nl", "nothing", Some(wrapped)),
        ent("ws", "Ws", "nothing", Some("  \n ")),
        ent("ex", "Ex", "nothing", Some(&exact)),
    ]
    .into_iter()
    .map(|e| (e.id.clone(), e))
    .collect();
    let g = build_graph(&ents);
    let m = build_manifest("proj", "", &ents, &g);
    let llms = render_llms_txt(&m);
    let md = render_index_md(&m);

    assert_eq!(uk_cut.chars().count(), 160);
    assert!(uk_cut.ends_with('…'));
    assert!(
        llms.contains(&format!("- [Uk](uk.md): {uk_cut}\n")),
        "{llms}"
    );
    assert!(md.contains(&format!("(in 0, out 0): {uk_cut}\n")), "{md}");

    assert!(
        llms.contains("- [Nl](nl.md): first line second line third\n"),
        "{llms}"
    );
    assert!(
        md.contains("(in 0, out 0): first line second line third\n"),
        "{md}"
    );

    assert!(llms.contains("- [Ws](ws.md)\n"), "{llms}");
    assert!(
        md.lines()
            .any(|l| l.contains("[Ws](ws.md)") && l.ends_with("(in 0, out 0)")),
        "{md}"
    );
    for out in [&llms, &md] {
        assert!(!out.contains(": \n"), "{out}");
        assert!(
            !out.lines().any(|l| l.starts_with(char::is_whitespace)),
            "{out}"
        );
    }

    let ex_line = format!("- [Ex](ex.md): {exact}\n");
    assert!(llms.contains(&ex_line), "{llms}");
    assert!(!ex_line.contains('…'));
}

#[test]
fn largest_fit_finds_the_boundary() {
    assert_eq!(largest_fit(10, |n| n <= 6), 6);
    assert_eq!(largest_fit(10, |_| true), 10);
    assert_eq!(largest_fit(10, |_| false), 0);
    assert_eq!(largest_fit(0, |_| true), 0);
}

#[test]
fn token_estimate_is_chars_over_four() {
    assert_eq!(token_estimate("abcdefgh"), 2);
}

#[test]
fn agents_md_documents_slug_piped_link_format() {
    let md = render_agents_md("proj");
    assert!(
        md.contains("[[slug|Name]]"),
        "AGENTS.md should describe the slug-piped link format: {md}"
    );
    assert!(
        !md.contains("`[[Name]]`"),
        "AGENTS.md still describes the old [[Name]] format: {md}"
    );
}

#[test]
fn index_json_carries_defined_and_methods_as_the_last_entry_keys() {
    let (mut ents, g) = setup();
    let alpha_e = ents.get_mut("alpha").unwrap();
    alpha_e.defined = vec!["Alpha".into(), "build_alpha".into()];
    alpha_e.methods = vec!["build".into()];
    let m = build_manifest("My Project", "", &ents, &g);
    let json = render_index_json(&m);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let alpha = &v["entries"][0];
    assert_eq!(alpha["id"], "alpha");
    assert_eq!(
        alpha["defined"],
        serde_json::json!(["Alpha", "build_alpha"])
    );
    assert_eq!(alpha["methods"], serde_json::json!(["build"]));
    assert_eq!(v["entries"][1]["defined"], serde_json::json!([]));
    assert_eq!(v["entries"][1]["methods"], serde_json::json!([]));
    // Key order in the pretty output: neighbors_in < defined < methods,
    // so `methods` is the last key of every entry.
    let a_text = &json[json.find("\"id\": \"alpha\"").unwrap()..];
    let pos_aliases = a_text.find("\"aliases\"").unwrap();
    let pos_tags = a_text.find("\"tags\"").unwrap();
    let pos_path = a_text.find("\"path\"").unwrap();
    assert!(pos_aliases < pos_tags && pos_tags < pos_path, "{json}");
    let pos_neighbors_in = a_text.find("\"neighbors_in\"").unwrap();
    let pos_defined = a_text.find("\"defined\"").unwrap();
    let pos_methods = a_text.find("\"methods\"").unwrap();
    assert!(
        pos_neighbors_in < pos_defined && pos_defined < pos_methods,
        "expected neighbors_in < defined < methods:\n{json}"
    );
}

#[test]
fn items_json_keys_each_code_page_by_id_and_skips_pages_without_items() {
    let (mut ents, _) = setup();
    ents.get_mut("beta").unwrap().items = vec![
        CodeItem {
            kind: CodeItemKind::Fn,
            name: "build_beta".into(),
            owner: None,
            signature: "pub fn build_beta()".into(),
            start_line: 1,
            end_line: 1,
            doc: None,
            doc_text: None,
            public: true,
            nested: false,
            twin: None,
            test: false,
            calls: Vec::new(),
            call_lines: Vec::new(),
            call_sites: Vec::new(),
            arity: None,
            pair_role: PairRole::Neither,
            namespace: None,
        },
        CodeItem {
            kind: CodeItemKind::Method,
            name: "build".into(),
            owner: Some("Beta".into()),
            signature: "fn build()".into(),
            start_line: 3,
            end_line: 5,
            doc: Some("Builds.".into()),
            doc_text: None,
            public: false,
            nested: false,
            twin: Some("beta.cpp:2-4".into()),
            test: true,
            calls: vec!["beta.rs:1-1".into()],
            call_lines: Vec::new(),
            call_sites: Vec::new(),
            arity: None,
            pair_role: PairRole::Neither,
            namespace: None,
        },
    ];
    // Each item's keys in declaration order, the `None` ones skipped;
    // `alpha` has no items, so it has no key.
    assert_eq!(
        render_items_json(&ents),
        concat!(
            r#"{"beta":["#,
            r#"{"kind":"fn","name":"build_beta","signature":"pub fn build_beta()","start_line":1,"end_line":1,"public":true},"#,
            r#"{"kind":"method","name":"build","owner":"Beta","signature":"fn build()","start_line":3,"end_line":5,"doc":"Builds.","public":false,"twin":"beta.cpp:2-4","test":true,"calls":["beta.rs:1-1"]}]}"#
        )
    );
}

#[test]
fn index_entry_has_updated_only_when_known() {
    let (mut ents, _) = setup();
    ents.get_mut("alpha").unwrap().updated = "2026-10-23".into();
    let g = build_graph(&ents);
    let v: serde_json::Value =
        serde_json::from_str(&render_index_json(&build_manifest("p", "", &ents, &g))).unwrap();
    assert_eq!(v["entries"][0]["updated"], "2026-10-23");
    assert!(v["entries"][1].get("updated").is_none());
}

#[test]
fn index_entry_has_memory_only_when_flagged() {
    let (mut ents, _) = setup();
    ents.get_mut("alpha").unwrap().memory = true;
    let g = build_graph(&ents);
    let v: serde_json::Value =
        serde_json::from_str(&render_index_json(&build_manifest("p", "", &ents, &g))).unwrap();
    assert_eq!(v["entries"][0]["memory"], true);
    assert!(v["entries"][1].get("memory").is_none());
}

#[test]
fn index_md_summaries_get_the_obsidian_fixups() {
    let (ents, g) = setup();
    let mut m = build_manifest("proj", "", &ents, &g);
    m.entries[0].summary = Some("see [[Beta]], [b](beta.txt) and #5-x".into());
    let md = render_index_md(&m);
    assert!(
        md.contains(": see [[beta]], [b](beta.md) and \\#5-x\n"),
        "{md}"
    );
}
