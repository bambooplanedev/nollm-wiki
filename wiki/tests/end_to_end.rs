//! Full `compile()` runs on small in-memory corpora: artifacts, cross-links, reserved-name remapping, incremental edit and delete, and byte-identical output across `--jobs`.

use std::collections::BTreeSet;
use std::fs;
use tempfile::tempdir;
use wiki::{compile, CompileOptions};

mod common;
use common::{exports, imports, write};

#[test]
fn compiles_a_small_corpus_and_emits_artifacts() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    write(&input, "alpha.txt", "# Alpha\n\nAlpha mentions Beta.\n");
    write(&input, "beta.txt", "# Beta\n\nBeta stands alone.\n");

    let result = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert_eq!(result.pages_total, 2);
    assert!(output.join("alpha.md").exists());
    assert!(output.join("index.json").exists());
    assert!(output.join("llms.txt").exists());
    assert!(output.join("AGENTS.md").exists());
    let alpha = fs::read_to_string(output.join("alpha.md")).unwrap();
    assert!(alpha.contains("[[beta|Beta]]"));
}

#[test]
fn output_is_deterministic_across_jobs() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    fs::create_dir_all(&input).unwrap();
    for i in 0..20 {
        write(
            &input,
            &format!("n{i}.txt"),
            &format!("# Node {i}\n\nNode {i} mentions Node {}.\n", (i + 1) % 20),
        );
    }
    // The .txt corpus never reaches formats/code.rs. Extraction walks the
    // syntax tree and resolves owners by ancestry, so the determinism test
    // has to include a file that exercises it.
    write(
        &input,
        "deep.rs",
        "//! Deep module.\npub struct Wiki;\nimpl Wiki {\n    pub fn search(&self, q: &str) -> u8 { 0 }\n}\nimpl Display for Wiki {\n    fn fmt(&self) {}\n}\npub const LIMIT: u32 = 5;\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn t() {}\n}\n",
    );
    // The Python path resolves owners by ancestry, gates on `__all__`, and
    // splices decorators — none of which the .txt or .rs fixtures exercise.
    // Written into the tempdir, so it never compiles into `.wiki/`.
    // Named deep_py.py (not deep.py) because deep.rs already slugs to "deep";
    // a colliding slug would silently drop one fixture and this test would
    // pass for the wrong reason.
    // MAX_IDS is listed in __all__ so the fixture genuinely exercises
    // constant extraction under the gate; `hidden` stays off the list so it
    // still pins the __all__ gate itself.
    write(
        &input,
        "deep_py.py",
        "__all__ = [\"Article\", \"build\", \"MAX_IDS\"]\nfrom .models import Base\n\nMAX_IDS = 2000\n\n@dataclass(frozen=True)\nclass Article:\n    title: str\n    url: str = \"\"\n\n    def render(self) -> str:\n        return \"\"\n\ndef build() -> Article:\n    def helper():\n        pass\n    return Article(\"\", \"\")\n\ndef hidden() -> None:\n    pass\n",
    );
    let out1 = dir.path().join("o1");
    let out2 = dir.path().join("o2");
    let mut o = CompileOptions {
        jobs: Some(1),
        ..Default::default()
    };
    compile(&input, &out1, &o).unwrap();
    o.jobs = Some(8);
    compile(&input, &out2, &o).unwrap();
    let a = fs::read_to_string(out1.join("index.json")).unwrap();
    let b = fs::read_to_string(out2.join("index.json")).unwrap();
    assert_eq!(a, b);

    let page_a = fs::read_to_string(out1.join("deep.md")).unwrap();
    let page_b = fs::read_to_string(out2.join("deep.md")).unwrap();
    assert_eq!(page_a, page_b);
    assert!(
        page_a.contains("pub fn Wiki::search(&self, q: &str) -> u8"),
        "page: {page_a}"
    );
    assert!(
        page_a.contains("fn <Wiki as Display>::fmt(&self)"),
        "page: {page_a}"
    );
    assert!(!page_a.contains("super::*"), "page: {page_a}");

    let py_a = fs::read_to_string(out1.join("deep_py.md")).unwrap();
    let py_b = fs::read_to_string(out2.join("deep_py.md")).unwrap();
    assert_eq!(py_a, py_b);
    assert!(
        py_a.contains("@dataclass(frozen=True) class Article"),
        "page: {py_a}"
    );
    assert!(py_a.contains("Article.title: str"), "page: {py_a}");
    assert!(
        py_a.contains("def Article.render(self) -> str"),
        "page: {py_a}"
    );
    // `## Body` is always the untouched raw source (only Rust's test-module
    // splice touches source pre-extraction), so a whole-page `contains`
    // pins nothing about extraction — it would pass even with import or
    // constant capture removed entirely. Scope to the curated sections.
    let py_exports = exports(&py_a);
    let py_imports = imports(&py_a);
    assert!(
        py_exports.contains(&"MAX_IDS = 2000".to_string()),
        "constant missing from Exports: {py_exports:?}"
    );
    assert!(
        py_imports.contains(&".models".to_string()),
        "import missing from Imports: {py_imports:?}"
    );
    assert!(
        !py_exports.iter().any(|s| s.contains("helper")),
        "function-local leak into Exports: {py_exports:?}"
    );
    assert!(
        !py_exports.iter().any(|s| s.contains("hidden")),
        "__all__ gate leak into Exports: {py_exports:?}"
    );
}

#[test]
fn source_slugging_to_reserved_manifest_name_is_remapped_not_clobbered() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    // This file's title slugs to "index", which collides with the manifest's
    // own index.md — it must be remapped, not silently overwritten.
    write(
        &input,
        "index.txt",
        "# Index\n\nsome body mentioning Alpha.\n",
    );
    write(&input, "alpha.txt", "# Alpha\n\nAlpha stands alone.\n");

    compile(&input, &output, &CompileOptions::default()).unwrap();

    // index.md must exist and be the MANIFEST index, not the source page.
    let index_md = fs::read_to_string(output.join("index.md")).unwrap();
    assert!(
        index_md.contains("Index") && index_md.contains("pages"),
        "index.md should be the manifest index page, got:\n{index_md}"
    );
    // A real *page* render (see rewrite::render_page) always carries this
    // compiler-owned marker and a "## Body" section; the manifest index
    // never does. If either shows up, the source page clobbered index.md.
    assert!(
        !index_md.contains("do not edit compiler-owned sections") && !index_md.contains("## Body"),
        "index.md contains page-render markers — the source page clobbered the manifest:\n{index_md}"
    );

    // The source entity's page must have been written under a non-reserved,
    // remapped name instead of clobbering index.md.
    let remapped = output.join("index_page.md");
    assert!(
        remapped.exists(),
        "expected the reserved-name source page to be written as index_page.md"
    );
    let remapped_content = fs::read_to_string(&remapped).unwrap();
    assert!(remapped_content.contains("some body mentioning Alpha"));

    // index.json must parse and list the remapped entity with a `page` field
    // that matches the file actually written to disk.
    let index_json = fs::read_to_string(output.join("index.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&index_json).unwrap();
    let entries = parsed["entries"].as_array().unwrap();
    let remapped_entry = entries
        .iter()
        .find(|e| e["id"] == "index_page")
        .expect("manifest should list the remapped 'index_page' entity");
    assert_eq!(remapped_entry["page"], "index_page.md");
}

#[test]
fn output_nested_under_input_does_not_ingest_generated_pages() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("proj");
    let output = input.join("wiki"); // output nested INSIDE input
    fs::create_dir_all(&input).unwrap();
    write(&input, "alpha.txt", "# Alpha\n\nAlpha stands alone.\n");

    // First compile: output dir does not exist yet, so only alpha.txt is seen.
    let r1 = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert_eq!(r1.pages_total, 1);

    // Second compile: output/*.md now exist under input. They must NOT be
    // ingested as source pages, or a watch loop would recompile forever.
    let r2 = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert_eq!(
        r2.pages_total, 1,
        "generated pages under the output dir were ingested as sources"
    );
}

#[test]
fn names_differing_only_by_case_collapse_to_one_page() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    // Two sources whose titles differ only by case both slugify to "widget".
    write(&input, "a.md", "---\ntitle: Widget\n---\n\nFirst body.\n");
    write(&input, "b.md", "---\ntitle: widget\n---\n\nSecond body.\n");

    let result = compile(&input, &output, &CompileOptions::default()).unwrap();

    // Deterministic dedup: exactly one entity, one page file, and it is the
    // lowercase slug "widget".
    assert_eq!(result.pages_total, 1, "case-only duplicate was not deduped");
    assert!(output.join("widget.md").exists(), "expected widget.md");

    // The kept page is the first by sorted rel_path (a.md -> "First body.").
    let page = fs::read_to_string(output.join("widget.md")).unwrap();
    assert!(
        page.contains("First body."),
        "expected the first-by-path source to win, got:\n{page}"
    );
}

#[test]
fn incremental_recompile_with_no_changes_writes_nothing() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    write(&input, "alpha.txt", "# Alpha\n\nAlpha mentions Beta.\n");
    write(&input, "beta.txt", "# Beta\n\nBeta stands alone.\n");

    let opts = CompileOptions {
        incremental: true,
        ..Default::default()
    };
    // Fresh build writes every page.
    let first = compile(&input, &output, &opts).unwrap();
    assert_eq!(first.pages_written, first.pages_total);

    // A recompile with no source changes must write nothing. The render
    // fingerprint includes the preserved Notes section; the fresh build emits
    // a Notes placeholder, so the fingerprint must treat that placeholder as
    // "no notes" — otherwise the first recompile re-reads the placeholder,
    // computes a different fingerprint, and rewrites every page.
    let second = compile(&input, &output, &opts).unwrap();
    assert_eq!(
        second.pages_written, 0,
        "no-change recompile rewrote {} pages",
        second.pages_written
    );
}

#[test]
fn same_named_files_in_different_directories_all_survive() {
    // The canonical Python package layout: a `models.py` and an `__init__.py`
    // per app. The `models.py` pair collides on the id `models`, and the dedup
    // in `compile_inner` used to drop the loser with nothing louder than a
    // stderr warning — the run still exited 0 and lint still reported no
    // broken links, so the loss was invisible to both CI and an agent.
    //
    // The `__init__.py` pair no longer collides: a directory-module file is
    // named for its directory, so these are `api` and `db` rather than two
    // pages both called "Init". That is the point of the naming rule — the
    // page for `app/api/__init__.py` IS the `api` package, and code refers to
    // it that way. Disambiguation for directory modules is still exercised
    // below, by two packages that genuinely share a directory name.
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    for app in ["api", "db"] {
        write(
            &input,
            &format!("app/{app}/__init__.py"),
            "\"\"\"Facade.\"\"\"\n",
        );
        write(
            &input,
            &format!("app/{app}/models.py"),
            "def handle():\n    \"\"\"Handle it.\"\"\"\n    return 1\n",
        );
    }

    let result = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert_eq!(result.pages_total, 4, "no file may be dropped");
    for id in ["api_models", "db_models", "api", "db"] {
        assert!(output.join(format!("{id}.md")).exists(), "missing {id}.md");
    }
    // The qualified title is what the page is headed with, not just its slug.
    let page = fs::read_to_string(output.join("api_models.md")).unwrap();
    assert!(page.starts_with("# Api Models\n"), "page head:\n{page}");
    // The directory-module page is headed with its directory's name.
    let page = fs::read_to_string(output.join("api.md")).unwrap();
    assert!(page.starts_with("# Api\n"), "page head:\n{page}");
}

#[test]
fn directory_modules_sharing_a_directory_name_are_still_disambiguated() {
    // Naming a directory module for its directory moves the collision rather
    // than removing it: two packages both called `api` still need qualifying,
    // and must not silently drop one another.
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    for top in ["app", "lib"] {
        write(
            &input,
            &format!("{top}/api/__init__.py"),
            "\"\"\"Facade.\"\"\"\n",
        );
    }
    let result = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert_eq!(result.pages_total, 2, "no file may be dropped");
    for id in ["app_api", "lib_api"] {
        assert!(output.join(format!("{id}.md")).exists(), "missing {id}.md");
    }
}

#[test]
fn a_unique_filename_keeps_its_short_page_id() {
    // Qualification must stay collision-driven: prefixing every page with its
    // directory would make every wikilink target long and every title with it.
    // (Deliberately not named `graph` — that slug is a reserved manifest name
    // and gets remapped by a different pass, which would mask this one.)
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    write(
        &input,
        "src/deep/nested/walker.py",
        "def build():\n    return 1\n",
    );

    compile(&input, &output, &CompileOptions::default()).unwrap();
    assert!(output.join("walker.md").exists());
}

/// alpha -> beta, plus gamma so the manifest has more than the two pages
/// under test. Returns the tempdir; input is `<dir>/raw`, output `<dir>/out`.
fn write_incremental_corpus() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    fs::create_dir_all(&input).unwrap();
    write(&input, "alpha.txt", "# Alpha\n\nAlpha mentions Beta.\n");
    write(&input, "beta.txt", "# Beta\n\nBeta stands alone.\n");
    write(&input, "gamma.txt", "# Gamma\n\nGamma stands alone.\n");
    dir
}

#[test]
fn incremental_edit_rewrites_only_that_page() {
    let dir = write_incremental_corpus();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    let opts = CompileOptions {
        incremental: true,
        ..Default::default()
    };
    compile(&input, &output, &opts).unwrap();
    let alpha_before = fs::read(output.join("alpha.md")).unwrap();
    let beta_before = fs::read(output.join("beta.md")).unwrap();

    // Extend beta's body past its first sentence so its summary (which
    // alpha's Related section repeats) stays the same and only beta's own
    // render changes.
    write(
        &input,
        "beta.txt",
        "# Beta\n\nBeta stands alone. Beta gained a second sentence.\n",
    );
    let r = compile(&input, &output, &opts).unwrap();
    assert_eq!(r.pages_written, 1, "expected only beta.md to be rewritten");
    assert_eq!(
        fs::read(output.join("alpha.md")).unwrap(),
        alpha_before,
        "alpha.md must be byte-identical after an unrelated edit"
    );
    assert_ne!(
        fs::read(output.join("beta.md")).unwrap(),
        beta_before,
        "beta.md must reflect the edited source"
    );
}

#[test]
fn incremental_delete_prunes_page_and_cache() {
    let dir = write_incremental_corpus();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    let opts = CompileOptions {
        incremental: true,
        ..Default::default()
    };
    compile(&input, &output, &opts).unwrap();
    assert!(output.join("beta.md").exists());
    let alpha = fs::read_to_string(output.join("alpha.md")).unwrap();
    assert!(alpha.contains("[[beta|Beta]]"), "fixture invalid:\n{alpha}");

    fs::remove_file(input.join("beta.txt")).unwrap();
    compile(&input, &output, &opts).unwrap();

    assert!(
        !output.join("beta.md").exists(),
        "deleted source's page must be pruned from disk"
    );
    // `.wiki/cache.json` is `cache::Cache`: `pages` is a map id -> fingerprint.
    let cache: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.join(".wiki/cache.json")).unwrap())
            .unwrap();
    let cached = cache["pages"].as_object().unwrap();
    assert!(
        !cached.contains_key("beta"),
        "deleted page must leave the cache, got keys {:?}",
        cached.keys().collect::<Vec<_>>()
    );
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.join("index.json")).unwrap()).unwrap();
    assert!(
        !index["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == "beta"),
        "deleted page must leave index.json"
    );
    // alpha linked to beta, so its render changed and it must be rewritten
    // without the now-dangling wikilink.
    let alpha = fs::read_to_string(output.join("alpha.md")).unwrap();
    assert!(
        !alpha.contains("[[beta"),
        "alpha.md still links the deleted page:\n{alpha}"
    );
}

#[test]
fn emit_json_writes_a_parsable_graph() {
    let dir = write_incremental_corpus();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    let opts = CompileOptions {
        emit_json: true,
        ..Default::default()
    };
    compile(&input, &output, &opts).unwrap();

    // `manifest::render_graph_json`: {"nodes": [{id, title, kind, pagerank}],
    // "edges": [{source, target}]}.
    let graph: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.join("graph.json")).unwrap()).unwrap();
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.join("index.json")).unwrap()).unwrap();
    let ids = |v: &serde_json::Value| -> BTreeSet<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect()
    };
    let node_ids = ids(&graph["nodes"]);
    assert_eq!(node_ids, ids(&index["entries"]));
    assert_eq!(node_ids.len(), 3);
    let edges = graph["edges"].as_array().unwrap();
    assert!(
        edges
            .iter()
            .any(|e| e["source"] == "alpha" && e["target"] == "beta"),
        "alpha -> beta edge missing: {edges:?}"
    );
}

#[test]
fn js_ts_go_extract_exported_signatures() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    write(
        &input,
        "mod.js",
        "export function foo(a) {\n  return a;\n}\nexport const K = 1;\nexport default foo(1);\nfunction hidden() {}\n",
    );
    write(
        &input,
        "svc.ts",
        "export class Svc {\n  run(): void {}\n}\nexport interface Shape {}\nconst x = 1;\n",
    );
    write(
        &input,
        "thing.go",
        "package thing\n\nfunc Exported() {}\nfunc unexported() {}\ntype Thing struct{}\nfunc (t Thing) Method() {}\n",
    );
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let page = |id: &str| fs::read_to_string(output.join(format!("{id}.md"))).unwrap();

    // JS (`extract_simple::js_spec`): only function/class declarations wrapped
    // in an `export_statement`. `export const`, `export default <expr>` and
    // the bare `function hidden` are deliberately not captured.
    let js = exports(&page("mod"));
    assert_eq!(js, vec!["export function foo(a)"], "js exports: {js:?}");

    // TS (`ts_spec`): same gate; `export interface` is not a class/function
    // declaration, so it is deliberately not captured either.
    let ts = exports(&page("svc"));
    assert_eq!(ts, vec!["export class Svc"], "ts exports: {ts:?}");

    // Go (`go_spec`): every func/type declaration, filtered to a leading
    // uppercase rune. Methods with receivers are `method_declaration`, not
    // `function_declaration`, so `Method` is deliberately not captured.
    let go = exports(&page("thing"));
    assert_eq!(
        go,
        vec!["func Exported()", "type Thing struct{}"],
        "go exports: {go:?}"
    );
}

#[test]
fn index_json_carries_methods_for_code_pages_and_empty_for_text() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    write(
        &input,
        "store.rs",
        "pub struct Store;\nimpl Store {\n    pub fn open() -> Store {\n        Store\n    }\n}\n",
    );
    write(&input, "notes.txt", "Plain notes.\n");
    let output = dir.path().join("out");
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.join("index.json")).unwrap()).unwrap();
    let entry = |id: &str| {
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("no entry {id}"))
            .clone()
    };
    assert_eq!(entry("store")["methods"], serde_json::json!(["open"]));
    assert_eq!(entry("notes")["methods"], serde_json::json!([]));
}

/// A relative markdown link is a reference as deliberate as a wikilink: it
/// becomes a graph edge when its target, resolved against the linking file's
/// directory, is another page's source path. External links and links to
/// nothing in the tree are ignored.
#[test]
fn relative_markdown_links_become_edges() {
    let tmp = tempdir().unwrap();
    let raw = tmp.path().join("raw");
    let out = tmp.path().join("out");
    fs::create_dir_all(raw.join("docs")).unwrap();
    fs::write(raw.join("README.md"), "# Root\n\nStart here.\n").unwrap();
    fs::write(
        raw.join("docs/guide.md"),
        "# Guide\n\nSee the [top page](../README.md#start), the [site](https://example.org/README.md), and [nothing](missing.md).\n",
    )
    .unwrap();
    let r = compile(&raw, &out, &CompileOptions::default()).unwrap();

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("index.json")).unwrap()).unwrap();
    let entry = |id: &str| {
        index["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(entry("guide")["neighbors_out"], serde_json::json!(["root"]));
    assert_eq!(entry("root")["neighbors_in"], serde_json::json!(["guide"]));
    assert!(
        !r.lint.orphans.contains(&"root".to_string()),
        "root is linked by guide, orphans: {:?}",
        r.lint.orphans
    );
    // The body never says "root", so only the link can have made the edge.
    let page = fs::read_to_string(out.join("guide.md")).unwrap();
    let body = page.split("## Body").nth(1).unwrap();
    assert!(!body.to_lowercase().contains("root"), "{body}");
}

/// A one-word code page is linked by a code-shaped mention (a `use` path,
/// here) and not by the same word in prose. Pinned through `index.json`
/// because that is what agents and `neighbors` read.
#[test]
fn prose_does_not_link_a_code_page_but_a_use_path_does() {
    let tmp = tempdir().unwrap();
    let raw = tmp.path().join("raw");
    let out = tmp.path().join("out");
    common::write(
        &raw,
        "src/text.rs",
        "//! Text extractor.\npub fn extract() {}\n",
    );
    common::write(
        &raw,
        "notes.md",
        "# Notes\n\nThe text of the page is long.\n",
    );
    common::write(
        &raw,
        "src/user.rs",
        "//! Uses the extractor.\nuse crate::text::extract;\npub fn run() { extract() }\n",
    );
    compile(&raw, &out, &CompileOptions::default()).unwrap();

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("index.json")).unwrap()).unwrap();
    let entry = |id: &str| {
        index["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(entry("user")["neighbors_out"], serde_json::json!(["text"]));
    assert_eq!(entry("notes")["neighbors_out"], serde_json::json!([]));
    assert_eq!(entry("text")["neighbors_in"], serde_json::json!(["user"]));
}
