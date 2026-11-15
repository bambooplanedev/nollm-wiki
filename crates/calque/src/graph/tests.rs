//! Tests for `graph`.
use super::*;
use crate::model::{Entity, SourceKind};
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

fn map(v: Vec<Entity>) -> BTreeMap<String, Entity> {
    v.into_iter().map(|e| (e.id.clone(), e)).collect()
}

fn code(id: &str, name: &str, path: &str, body: &str) -> Entity {
    let mut e = ent(id, name, body);
    e.source_path = path.into();
    e.kind = SourceKind::Code {
        lang: "rust".into(),
    };
    e
}

#[test]
fn refers_to_module_accepts_paths_declarations_and_filenames_only() {
    for yes in [
        "text::extract(x)",
        "use crate::text::Extract;",
        "see ::text for details",
        "mod text;",
        "pub mod text {",
        "pub(crate) mod   text;",
        "lives in src/text.rs",
        "text.rs",
    ] {
        assert!(refers_to_module(yes, "text", "rs"), "should match: {yes}");
    }
    for no in [
        "the text of the page",
        "body_text::x",
        "text_extractor::run()",
        "in context::here",
        "`text`",
        "SourceKind::Text",
        "ContentBlock::text(\"x\")",
        "text.rsx",
        "text.json",
        "method text;",
        "",
    ] {
        assert!(
            !refers_to_module(no, "text", "rs"),
            "should not match: {no}"
        );
    }
    // The extension is the target's own, not a fixed list.
    assert!(refers_to_module("from models.py", "models", "py"));
    assert!(!refers_to_module("from models.py", "models", "rs"));
}

#[test]
fn a_prose_mention_of_a_code_page_is_not_an_edge_but_a_path_is() {
    let text = code("text", "Text", "src/text.rs", "");
    let notes = ent("notes", "Notes", "the text of the page is long");
    let mut user = code("user", "User", "src/user.rs", "use crate::text::Extract;");
    user.imports = vec![]; // pin the filter, not the resolver
    let g = build_graph(&map(vec![text, notes, user]));
    assert!(
        !g.edges["notes"].outgoing.contains("text"),
        "{:?}",
        g.edges["notes"]
    );
    assert!(
        g.edges["user"].outgoing.contains("text"),
        "{:?}",
        g.edges["user"]
    );
    assert!(g.edges["text"].incoming.contains("user"));
}

#[test]
fn multi_word_code_titles_are_filtered_too() {
    let er = code(
        "extract_rust",
        "Extract Rust",
        "src/formats/extract_rust.rs",
        "",
    );
    let prose = ent("a", "Alpha", "Extract Rust handles pub gating");
    let file = ent("b", "Beta", "gating lives in extract_rust.rs today");
    let g = build_graph(&map(vec![er, prose, file]));
    assert!(!g.edges["a"].outgoing.contains("extract_rust"));
    assert!(g.edges["b"].outgoing.contains("extract_rust"));
}

#[test]
fn text_and_markdown_targets_keep_prose_mentions() {
    let target = ent("text", "Text", "a note titled Text");
    let src = ent("a", "Alpha", "the text of this note");
    let g = build_graph(&map(vec![target, src]));
    assert!(g.edges["a"].outgoing.contains("text"));
}

#[test]
fn bidirectional_edge_created() {
    let g = build_graph(&map(vec![
        ent("a", "Alpha", "mentions Beta here"),
        ent("b", "Beta", "nothing"),
    ]));
    assert!(g.edges["a"].outgoing.contains("b"));
    assert!(g.edges["b"].incoming.contains("a"));
}

#[test]
fn no_self_link() {
    let g = build_graph(&map(vec![ent("a", "Alpha", "Alpha refers to itself")]));
    assert!(!g.edges["a"].outgoing.contains("a"));
}

#[test]
fn alias_creates_edge() {
    let mut beta = ent("b", "Beta", "nothing");
    beta.aliases = vec!["Second Letter".into()];
    let g = build_graph(&map(vec![ent("a", "Alpha", "see the second letter"), beta]));
    assert!(g.edges["a"].outgoing.contains("b"));
}

#[test]
fn cyrillic_alias_creates_edge() {
    let mut ai = ent("ai", "Artificial intelligence", "About learning.");
    ai.aliases = vec!["ШІ".into()];
    let notes = ent("notes", "Notes", "Today we talked about ШІ.");
    let g = build_graph(&map(vec![ai, notes]));
    assert!(g.edges["notes"].outgoing.contains("ai"));
}

#[test]
fn accented_word_is_not_split_into_a_false_mention() {
    let ve = ent("ve", "Ve", "Short.");
    let trip = ent("trip", "Trip", "We saw a naïve painting.");
    let g = build_graph(&map(vec![ve, trip]));
    assert!(!g.edges["trip"].outgoing.contains("ve"));
}

#[test]
fn resolve_link_walks_relative_paths_and_skips_external_targets() {
    assert_eq!(
        resolve_link("scripts/README.md", "../README.md#self-hosting").as_deref(),
        Some("README.md")
    );
    assert_eq!(
        resolve_link("README.md", "wiki/docs/ARCHITECTURE.md").as_deref(),
        Some("wiki/docs/ARCHITECTURE.md")
    );
    assert_eq!(
        resolve_link("a/b/c.md", "./../d.md").as_deref(),
        Some("a/d.md")
    );
    assert_eq!(resolve_link("README.md", "../outside.md"), None);
    assert_eq!(resolve_link("README.md", "#anchor"), None);
    assert_eq!(resolve_link("README.md", "https://example.org/x.md"), None);
    assert_eq!(
        resolve_link("README.md", "mailto:someone@example.org"),
        None
    );
}

#[test]
fn markdown_link_to_a_source_path_creates_an_edge_without_a_mention() {
    let mut a = ent(
        "a",
        "Alpha",
        "see [the other page](docs/b.md \"title\") and [x](nope.md)",
    );
    a.source_path = "README.md".into();
    let mut b = ent("b", "Beta", "nothing");
    b.source_path = "docs/b.md".into();
    let g = build_graph(&map(vec![a, b]));
    assert!(g.edges["a"].outgoing.contains("b"));
    assert!(g.edges["b"].incoming.contains("a"));
    assert!(g.edges["b"].outgoing.is_empty());
}

#[test]
fn pagerank_ignores_memory_pages() {
    let a = ent("a", "Alpha", "nothing");
    let b = ent("b", "Beta", "mentions Alpha");
    let without = build_graph(&map(vec![a.clone(), b.clone()]));
    let mut m = ent("m", "Memo", "Alpha Alpha Alpha");
    m.memory = true;
    let with = build_graph(&map(vec![a, b, m]));
    assert!(with.edges["a"].incoming.contains("m"));
    assert_eq!(with.pagerank["a"], without.pagerank["a"]);
    assert_eq!(with.pagerank.get("m"), None);
}

#[test]
fn pagerank_is_deterministic_and_sums_to_about_one() {
    let g = build_graph(&map(vec![
        ent("a", "Alpha", "mentions Beta"),
        ent("b", "Beta", "mentions Alpha"),
    ]));
    let sum: f64 = g.pagerank.values().sum();
    assert!((sum - 1.0).abs() < 1e-6);
}

fn sh(id: &str, path: &str, imports: &[&str]) -> Entity {
    let mut e = code(id, id, path, "");
    e.kind = SourceKind::Code {
        lang: "shell".into(),
    };
    e.imports = imports.iter().map(|s| (*s).to_string()).collect();
    e
}

fn imports_of(files: Vec<Entity>, id: &str) -> Vec<String> {
    let g = build_graph(&map(files));
    g.edges[id].imports.iter().cloned().collect()
}

#[test]
fn source_literal_path_gives_one_edge() {
    let files = vec![
        sh(
            "main",
            "main.sh",
            &["lib/utils.sh", "./lib/utils.sh", "\"lib/utils.sh\""],
        ),
        sh("utils", "lib/utils.sh", &[]),
        sh("lib", "lib.sh", &[]),
        sh("sh", "sh.sh", &[]),
        sh("other", "x/lib/utils.sh", &[]),
    ];
    // Exact path wins over the suffix match that would be ambiguous.
    assert_eq!(imports_of(files, "main"), ["utils"]);
}

#[test]
fn source_literal_prefers_the_project_root_over_the_importing_dir() {
    let files = vec![
        sh("c", "bin/c.sh", &["lib/y.sh"]),
        sh("y", "lib/y.sh", &[]),
        sh("y2", "bin/lib/y.sh", &[]),
    ];
    assert_eq!(imports_of(files, "c"), ["y"]);
}

#[test]
fn source_literal_is_relative_to_the_importing_dir() {
    let files = vec![
        sh("main", "bin/main.sh", &["../lib/a.sh", "b.sh"]),
        sh("a", "lib/a.sh", &[]),
        sh("b", "bin/b.sh", &[]),
        sh("b2", "other/b.sh", &[]),
    ];
    assert_eq!(imports_of(files, "main"), ["a", "b"]);
}

#[test]
fn source_tail_after_variable_resolves() {
    for imp in [
        "\"${ROOT}\"/lib/x.sh",
        "$DIR/lib/x.sh",
        "\"$(dirname \"$0\")/lib/x.sh\"",
    ] {
        let files = vec![sh("main", "main.sh", &[imp]), sh("x", "lib/x.sh", &[])];
        assert_eq!(imports_of(files, "main"), ["x"], "{imp}");
    }
}

#[test]
fn source_ambiguous_or_empty_tail_gives_no_edge() {
    let files = vec![
        sh("main", "main.sh", &["$DIR/x.sh", "\"$1\"", "$DIR/"]),
        sh("a", "a/x.sh", &[]),
        sh("b", "b/x.sh", &[]),
    ];
    assert_eq!(imports_of(files, "main"), Vec::<String>::new());
}

#[test]
fn python_import_does_not_link_shell() {
    let mut app = code("app", "app", "app.py", "");
    app.imports = vec!["deploy".into(), "helper".into()];
    let mut d = sh("deploy", "deploy.sh", &[]);
    d.defined = vec!["helper".into()];
    assert_eq!(imports_of(vec![app, d], "app"), Vec::<String>::new());
}

#[test]
fn imports_resolve_every_segment_by_stem_and_defined_name() {
    let mut model = code("model", "Model", "wiki/src/model.rs", "");
    model.defined = vec!["Entity".into(), "Graph".into()];
    let formats = code("formats", "Formats", "wiki/src/formats/mod.rs", "");
    let codepage = code("code", "Code", "wiki/src/formats/code.rs", "");
    let mut summary = code("summary", "Summary", "wiki/src/formats/summary.rs", "");
    summary.defined = vec!["summarize".into()];
    let mut src_query = code("src_query", "Src Query", "wiki/src/query.rs", "");
    src_query.defined = vec!["Wiki".into(), "PackBudget".into()];
    let tests_query = code("tests_query", "Tests Query", "wiki/tests/query.rs", "");
    let mut lib = code("lib", "Lib", "wiki/src/lib.rs", "");
    lib.defined = vec!["compile".into(), "CompileOptions".into()];
    let readme = ent("wiki", "wiki", "the wiki README");
    let mut user = code("user", "User", "wiki/src/user.rs", "");
    user.imports = vec![
        "crate::model::{Entity, Graph}".into(),
        "crate::formats::code::CodeExtractor".into(),
        "crate::formats::{summarize, Extractor}".into(),
        "wiki::query::Wiki".into(),
        "wiki::{compile, CompileOptions}".into(),
        "rmcp::model::CallToolResult".into(),
        "std::collections::BTreeMap".into(),
        "serde::Serialize".into(),
    ];
    let g = build_graph(&map(vec![
        model,
        formats,
        codepage,
        summary,
        src_query,
        tests_query,
        lib,
        readme,
        user,
    ]));
    let out: Vec<&str> = g.edges["user"]
        .outgoing
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(
        out,
        ["code", "formats", "lib", "model", "src_query", "summary"],
        "src/ path wins the shared stem `query`; struct names resolve through `defined`; \
             rmcp/std/serde roots resolve nothing; the README is never an import target"
    );
}

#[test]
fn import_edges_are_kept_apart_from_mentions() {
    let text = code("text", "Text", "pkg/text.py", "");
    let mut consumer = code("consumer", "Consumer", "pkg/consumer.py", "");
    consumer.imports = vec![".text".into()];
    let notes = ent("notes", "Notes", "see `text.py`");
    let g = build_graph(&map(vec![text, consumer, notes]));
    assert_eq!(g.edges["consumer"].imports, g.edges["consumer"].outgoing);
    assert!(g.edges["notes"].outgoing.contains("text"));
    assert!(g.edges["notes"].imports.is_empty());
    let t = &g.edges["text"];
    assert_eq!(t.incoming.len(), 2);
    assert_eq!(t.imported_by.iter().collect::<Vec<_>>(), ["consumer"]);
}

#[test]
fn dotted_and_relative_imports_resolve_every_segment() {
    let text = code("text", "Text", "pkg/text.py", "");
    let mut consumer = code("consumer", "Consumer", "pkg/consumer.py", "");
    consumer.imports = vec![".text".into(), "os.path".into()];
    let g = build_graph(&map(vec![text, consumer]));
    assert!(g.edges["consumer"].outgoing.contains("text"));
    assert_eq!(g.edges["consumer"].outgoing.len(), 1);
}

#[test]
fn a_defined_name_shared_by_two_pages_resolves_nothing() {
    let mut a = code("a", "Alpha", "src/a.rs", "");
    a.defined = vec!["Thing".into()];
    let mut b = code("b", "Beta", "src/b.rs", "");
    b.defined = vec!["Thing".into()];
    let mut user = code("user", "User", "src/user.rs", "");
    user.imports = vec!["crate::Thing".into()];
    let g = build_graph(&map(vec![a, b, user]));
    assert!(g.edges["user"].outgoing.is_empty());
}

#[test]
fn a_ts_index_import_reaches_the_module_not_a_test_of_the_same_name() {
    // `backends` is the directory module; a test file named after it must
    // not take the stem and collect the importer's edge.
    let module = code("src_backends", "Backends", "src/backends/index.ts", "");
    let test = code(
        "tests_backends",
        "Tests Backends",
        "tests/backends.test.ts",
        "",
    );
    let mut app = code("app", "App", "src/app.ts", "");
    app.imports = vec!["./backends/index.js".into()];
    let g = build_graph(&map(vec![module, test, app]));
    let out: Vec<&str> = g.edges["app"].outgoing.iter().map(String::as_str).collect();
    assert_eq!(out, ["src_backends"]);
}
