//! Full `compile()` runs on small in-memory corpora: artifacts, cross-links, reserved-name remapping, incremental edit and delete, and byte-identical output across `--jobs`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use tempfile::{tempdir, TempDir};
use wiki::{compile, CompileOptions};

mod common;
use common::{exports, imports, index_entries, write};

#[test]
fn compiles_a_small_corpus_and_emits_artifacts() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    write(&input, "alpha.txt", "# Alpha\n\nAlpha mentions Beta.\n");
    write(&input, "beta.txt", "# Beta\n\nBeta stands alone.\n");

    let result = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert_eq!(result.pages_total, 2);
    assert!(output.join("alpha.md").exists());
    assert!(output.join("index.json").exists());
    assert_eq!(fs::read_to_string(output.join("items.json")).unwrap(), "{}");
    assert!(output.join("llms.txt").exists());
    assert!(output.join("AGENTS.md").exists());
    let alpha = fs::read_to_string(output.join("alpha.md")).unwrap();
    assert!(alpha.contains("[[beta|Beta]]"));
}

#[test]
fn output_is_deterministic_across_jobs() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
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
    let remapped_entry = index_entries(&output)
        .into_iter()
        .find(|e| e["id"] == "index_page")
        .expect("manifest should list the remapped 'index_page' entity");
    assert_eq!(remapped_entry["page"], "index_page.md");
}

#[test]
fn output_nested_under_input_does_not_ingest_generated_pages() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("proj");
    let output = input.join("wiki"); // output nested INSIDE input
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
fn names_differing_only_by_case_both_get_pages() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    // Two sources whose titles differ only by case both slugify to "widget".
    write(&input, "a.md", "---\ntitle: Widget\n---\n\nFirst body.\n");
    write(&input, "b.md", "---\ntitle: widget\n---\n\nSecond body.\n");

    let result = compile(&input, &output, &CompileOptions::default()).unwrap();

    // The escalation ladder makes disambiguation total: two distinct source
    // files always get two distinct pages. Neither basename's own dot count
    // nor source_rank differs (both are plain .md files), so the winner
    // exemption is a tie and both fall to the filename discriminator — the
    // basename stem, since these are prose pages named from their own title.
    assert_eq!(result.pages_total, 2, "both source files must get a page");
    assert!(
        !output.join("widget.md").exists(),
        "widget.md is ambiguous and must not be written"
    );
    assert!(output.join("widget_a.md").exists(), "expected widget_a.md");
    assert!(output.join("widget_b.md").exists(), "expected widget_b.md");

    let page_a = fs::read_to_string(output.join("widget_a.md")).unwrap();
    let page_b = fs::read_to_string(output.join("widget_b.md")).unwrap();
    assert!(page_a.contains("First body."), "a.md's body: {page_a}");
    assert!(page_b.contains("Second body."), "b.md's body: {page_b}");
}

#[test]
fn incremental_recompile_with_no_changes_writes_nothing() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
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
    // per app. The `models.py` pair collides on the id `models`, and a loser
    // dropped with nothing louder than a stderr warning would leave exit 0
    // and clean lint, invisible to both CI and an agent.
    //
    // The `__init__.py` pair does not collide: a directory-module file is
    // named for its directory, so these are `api` and `db` rather than two
    // pages both called "Init". That is the point of the naming rule — the
    // page for `app/api/__init__.py` IS the `api` package, and code refers to
    // it that way. Disambiguation for directory modules is still exercised
    // below, by two packages that genuinely share a directory name.
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
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
fn write_incremental_corpus() -> TempDir {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
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
    assert!(
        !index_entries(&output).iter().any(|e| e["id"] == "beta"),
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
    let index = common::index(&output);
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
    let (_dir, _, output) = common::compiled(&[
        ("mod.js", "export function foo(a) {\n  return a;\n}\nexport const K = 1;\nexport default foo(1);\nfunction hidden() {}\n"),
        ("svc.ts", "export class Svc {\n  run(): void {}\n}\nexport interface Shape {}\nconst x = 1;\n"),
        ("thing.go", "package thing\n\nfunc Exported() {}\nfunc unexported() {}\ntype Thing struct{}\nfunc (t Thing) Method() {}\n"),
    ]);
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
    let (_dir, _, output) = common::compiled(&[
        (
            "store.rs",
            "pub struct Store;\nimpl Store {\n    pub fn open() -> Store {\n        Store\n    }\n}\n",
        ),
        ("notes.txt", "Plain notes.\n"),
    ]);
    let entry = |id: &str| common::entry(&output, id);
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
    write(&raw, "README.md", "# Root\n\nStart here.\n");
    write(
        &raw,
        "docs/guide.md",
        "# Guide\n\nSee the [top page](../README.md#start), the [site](https://example.org/README.md), and [nothing](missing.md).\n",
    );
    let r = compile(&raw, &out, &CompileOptions::default()).unwrap();

    let entry = |id: &str| common::entry(&out, id);
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
    write(
        &raw,
        "src/text.rs",
        "//! Text extractor.\npub fn extract() {}\n",
    );
    write(
        &raw,
        "notes.md",
        "# Notes\n\nThe text of the page is long.\n",
    );
    write(
        &raw,
        "src/user.rs",
        "//! Uses the extractor.\nuse crate::text::extract;\npub fn run() { extract() }\n",
    );
    compile(&raw, &out, &CompileOptions::default()).unwrap();

    let entry = |id: &str| common::entry(&out, id);
    assert_eq!(entry("user")["neighbors_out"], serde_json::json!(["text"]));
    assert_eq!(entry("notes")["neighbors_out"], serde_json::json!([]));
    assert_eq!(entry("text")["neighbors_in"], serde_json::json!(["user"]));
}

#[test]
fn notes_follow_the_source_file_when_two_pages_swap_ids() {
    let src = tempdir().unwrap();
    let out = tempdir().unwrap();
    fs::write(src.path().join("x.md"), "# Alpha\n\nbody x\n").unwrap();
    fs::write(src.path().join("y.md"), "# Beta\n\nbody y\n").unwrap();

    let opts = CompileOptions {
        incremental: true,
        ..Default::default()
    };
    compile(src.path(), out.path(), &opts).unwrap();

    // Hand-write notes on each page.
    for (id, note) in [("alpha", "NOTE-FROM-X"), ("beta", "NOTE-FROM-Y")] {
        set_notes(&out.path().join(format!("{id}.md")), note);
    }

    // Swap the titles: x.md is now "Beta", y.md is now "Alpha".
    fs::write(src.path().join("x.md"), "# Beta\n\nbody x\n").unwrap();
    fs::write(src.path().join("y.md"), "# Alpha\n\nbody y\n").unwrap();
    compile(src.path(), out.path(), &opts).unwrap();

    let beta = fs::read_to_string(out.path().join("beta.md")).unwrap();
    let alpha = fs::read_to_string(out.path().join("alpha.md")).unwrap();
    assert!(
        beta.contains("body x"),
        "beta.md should now be sourced from x.md"
    );
    assert!(
        beta.contains("NOTE-FROM-X"),
        "notes must follow x.md, not the id 'beta': {beta}"
    );
    assert!(
        alpha.contains("NOTE-FROM-Y"),
        "notes must follow y.md, not the id 'alpha': {alpha}"
    );
}

#[test]
fn every_source_file_gets_a_page() {
    let (_dir, _, out) = common::compiled(&[
        ("a/foo.h", "class Foo {};\n"),
        ("a/foo.cpp", "int main(){return 0;}\n"),
        ("b/foo.h", "class Foo {};\n"),
        ("api/client.ts", "export function go(){}\n"),
        ("api/client.test.ts", "export function t(){}\n"),
        ("docs/v1.md", "# API Reference\n\nfirst\n"),
        ("docs/v2.md", "# API Reference\n\nsecond\n"),
    ]);
    // .h files only count once Task 8 lands; assert on what is handled today
    // and tighten the number in Task 12.
    let entries = index_entries(&out);
    let ids: Vec<&str> = entries.iter().map(|e| e["id"].as_str().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        ids.len(),
        "duplicate ids in index.json: {ids:?}"
    );
    assert!(ids.contains(&"client"), "ids: {ids:?}");
    assert!(ids.contains(&"client_test"), "ids: {ids:?}");
    assert!(
        ids.iter()
            .filter(|i| i.starts_with("api_reference"))
            .count()
            == 2,
        "both same-titled docs need pages: {ids:?}"
    );
}

#[test]
fn stale_pages_are_pruned_without_incremental_but_notes_are_never_destroyed() {
    let src = tempdir().unwrap();
    let out = tempdir().unwrap();
    fs::write(src.path().join("keep.md"), "# Keep\n\nbody\n").unwrap();
    fs::write(src.path().join("gone.md"), "# Gone\n\nbody\n").unwrap();
    fs::write(src.path().join("precious.md"), "# Precious\n\nbody\n").unwrap();

    // A plain compile records what it wrote, so no cache surgery is needed to
    // set this up: the two orphans below are pages this compiler really made.
    compile(src.path(), out.path(), &CompileOptions::default()).unwrap();
    let orphan = out.path().join("gone.md");
    let precious = out.path().join("precious.md");
    assert!(orphan.exists() && precious.exists());

    // One orphan-to-be carries REAL notes; the other keeps the placeholder.
    set_notes(&precious, "HAND WRITTEN");

    fs::remove_file(src.path().join("gone.md")).unwrap();
    fs::remove_file(src.path().join("precious.md")).unwrap();

    // Second plain compile — still no --incremental.
    compile(src.path(), out.path(), &CompileOptions::default()).unwrap();

    assert!(!orphan.exists(), "a placeholder-only orphan must be pruned");
    assert!(
        precious.exists(),
        "an orphan carrying hand-written notes must be kept, not deleted"
    );
}

/// Overwrite `## Notes` on an already-written page, asserting the placeholder
/// was there to replace.
fn set_notes(page: &Path, note: &str) {
    let text = fs::read_to_string(page).unwrap();
    let replaced = text.replace("_(add your own notes here — preserved on recompile)_", note);
    assert_ne!(
        text,
        replaced,
        "placeholder not found in {}",
        page.display()
    );
    fs::write(page, replaced).unwrap();
}

/// An orphan page left by an earlier id scheme still declares the live
/// source. If it wins the source-path index, the live page's own notes are
/// read out of the dead file and destroyed on the next compile. Default
/// (non-incremental) compiles throughout — this is the plain workflow.
#[test]
fn an_orphan_page_declaring_a_live_source_cannot_destroy_that_page_s_notes() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    let source = input.join("note.md");
    fs::write(&source, "---\ntitle: Alpha One\n---\n\nAlpha body.\n").unwrap();

    compile(&input, &output, &CompileOptions::default()).unwrap();
    assert!(output.join("alpha_one.md").exists());

    // Retitle: the page's id changes, orphaning alpha_one.md.
    fs::write(&source, "---\ntitle: Alpha Two\n---\n\nAlpha body.\n").unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let alpha_two = output.join("alpha_two.md");
    assert!(alpha_two.exists());

    // The user writes real notes on the LIVE page.
    set_notes(&alpha_two, "MY-REAL-NOTES");

    compile(&input, &output, &CompileOptions::default()).unwrap();
    let text = fs::read_to_string(&alpha_two).unwrap();
    assert!(
        text.contains("MY-REAL-NOTES"),
        "a live page's own notes must survive a recompile, even while an \
         orphan page declares the same source: {text}"
    );
}

/// The transplant half of the same defect: an orphan's notes must never be
/// written onto a different, live page. Spec Goal 3 — notes are "never
/// silently transplanted onto a different page".
#[test]
fn an_orphan_page_s_notes_are_never_transplanted_onto_the_live_page() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("note.md"),
        "---\ntitle: Alpha Two\n---\n\nAlpha body.\n",
    )
    .unwrap();

    compile(&input, &output, &CompileOptions::default()).unwrap();
    let alpha_two = output.join("alpha_two.md");
    set_notes(&alpha_two, "LIVE-NOTES-KEEP-ME");

    // A page from a previous id scheme, still declaring the live source and
    // sorting ahead of the live page's own file.
    let alpha_one = output.join("alpha_one.md");
    fs::write(
        &alpha_one,
        "# Alpha One\n\n## Metadata\n- source: note.md\n\n## Notes\nSTALE-ORPHAN-NOTES\n",
    )
    .unwrap();

    compile(&input, &output, &CompileOptions::default()).unwrap();
    let text = fs::read_to_string(&alpha_two).unwrap();
    assert!(
        text.contains("LIVE-NOTES-KEEP-ME"),
        "the live page must keep its own notes: {text}"
    );
    assert!(
        !text.contains("STALE-ORPHAN-NOTES"),
        "an orphan's notes must never be transplanted onto a live page: {text}"
    );
}

/// Pruning must be effective on a plain compile, not merely called: a
/// workflow that never passes `--incremental` still leaves orphans behind if
/// the record of what was written is only kept for incremental users.
#[test]
fn two_plain_compiles_prune_a_page_whose_source_was_deleted() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    write(&input, "keep.md", "# Keep\n\nbody\n");
    write(&input, "gone.md", "# Gone\n\nbody\n");

    compile(&input, &output, &CompileOptions::default()).unwrap();
    let orphan = output.join("gone.md");
    assert!(orphan.exists(), "first compile must write the page");

    fs::remove_file(input.join("gone.md")).unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();

    assert!(
        !orphan.exists(),
        "a plain compile must prune the page of a deleted source"
    );
    assert!(output.join("keep.md").exists(), "the live page must remain");
}

#[test]
fn items_json_round_trips() {
    use std::collections::BTreeMap;
    use wiki::formats::{CodeExtractor, Extractor};
    use wiki::model::CodeItem;
    let dir = tempdir().unwrap();
    let input = dir.path().join("in");
    let src = "pub fn live() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
    write(&input, "a.rs", src);
    let out = dir.path().join("out");
    compile(&input, &out, &CompileOptions::default()).unwrap();
    let json = fs::read_to_string(out.join("items.json")).unwrap();
    let back: BTreeMap<String, Vec<CodeItem>> = serde_json::from_str(&json).unwrap();
    // `arity` serves call matching only and is not written.
    let mut fresh = CodeExtractor.extract("a.rs", src).items;
    for i in &mut fresh {
        i.arity = None;
    }
    assert_eq!(back["a"], fresh);
    assert!(back["a"].iter().any(|i| i.test));
    // Only the test module and its fn carry the key.
    assert_eq!(json.matches("\"test\"").count(), 2, "{json}");
}

#[test]
fn incremental_compile_rerenders_on_tag_edit() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    let opts = CompileOptions {
        incremental: true,
        ..Default::default()
    };
    write(&input, "n.md", "---\ntags: [a]\n---\n# N\nbody\n");
    compile(&input, &output, &opts).unwrap();
    assert!(fs::read_to_string(output.join("n.md"))
        .unwrap()
        .contains("- tags: a"));
    write(&input, "n.md", "---\ntags: [b]\n---\n# N\nbody\n");
    compile(&input, &output, &opts).unwrap();
    assert!(fs::read_to_string(output.join("n.md"))
        .unwrap()
        .contains("- tags: b"));
}

/// `Канал` and `Kanal` slug to the same ascii stem; both keep a page (the
/// link `[[Канал]]` is then ambiguous, like any shared slug). A Cyrillic title
/// with a stem of its own is linked by its wikilink.
#[test]
fn cyrillic_titles_get_distinct_pages_and_resolve_links() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    write(&input, "a.md", "---\ntitle: Канал\n---\n\nCyrillic body.\n");
    write(&input, "b.md", "---\ntitle: Kanal\n---\n\nLatin body.\n");
    write(&input, "c.md", "---\ntitle: Third\n---\n\nSee [[Маяк]].\n");
    write(&input, "d.md", "---\ntitle: Маяк\n---\n\nPlan.\n");

    let result = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert_eq!(result.pages_total, 4);

    let entries = index_entries(&output);
    let ids: BTreeSet<&str> = entries.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        BTreeSet::from(["kanal_a", "kanal_b", "third", "maiak"])
    );
    let third = entries.iter().find(|e| e["id"] == "third").unwrap();
    assert_eq!(third["neighbors_out"], serde_json::json!(["maiak"]));
}

#[test]
fn repo_memory_folder_compiles_flagged_and_project_pages_are_not() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    write(&input, "doc.md", "---\ntitle: Doc\n---\n\nProject doc.\n");
    write(
        &input,
        ".agents/memory/note.md",
        "---\ntitle: Note\n---\n\nRemember this.\n",
    );
    write(&input, ".agents/memory/MEMORY.md", "# Index\n\n- note\n");
    write(&input, ".agents/memory/memory.md", "# Lower\n");
    write(&input, ".agents/memory/._n.md", "# Junk\n");

    let result = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert_eq!(result.pages_total, 2);
    let entries = index_entries(&output);
    let note = entries.iter().find(|e| e["title"] == "Note").unwrap();
    assert_eq!(note["path"], ".agents/memory/note.md");
    assert_eq!(note["memory"], true);
    let doc = entries.iter().find(|e| e["title"] == "Doc").unwrap();
    assert!(doc.get("memory").is_none());
    // The flag reaches index.json only, never the page text.
    let page = fs::read_to_string(output.join("note.md")).unwrap();
    assert!(!page.contains("memory: true"));
}

#[test]
fn memory_folder_excluded_by_git_is_still_compiled() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    write(&input, "doc.md", "# Doc\n\nbody\n");
    write(&input, ".agents/memory/note.md", "# Note\n\nkept\n");
    write(&input, ".git/info/exclude", ".agents/memory\n");
    write(&input, ".gitignore", ".agents/\n");

    compile(&input, &output, &CompileOptions::default()).unwrap();
    let entries = index_entries(&output);
    assert!(entries
        .iter()
        .any(|e| e["path"] == ".agents/memory/note.md" && e["memory"] == true));
}

#[test]
fn external_memory_folder_compiles_with_its_absolute_path() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let external = dir.path().join("ext");
    let output = dir.path().join("out");
    write(&input, "doc.md", "# Doc\n\nbody\n");
    write(&external, "n.md", "# Ext Note\n\nbody\n");

    let opts = CompileOptions {
        memory: Some(external.clone()),
        ..Default::default()
    };
    let result = compile(&input, &output, &opts).unwrap();
    assert_eq!(result.pages_total, 2);
    let entries = index_entries(&output);
    let note = entries.iter().find(|e| e["title"] == "Ext Note").unwrap();
    assert_eq!(
        note["path"],
        wiki::memory::display_path(&external.join("n.md"))
    );
    assert_eq!(note["memory"], true);
}

#[test]
fn memory_folder_given_twice_or_with_no_ignore_yields_each_note_once() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    write(&input, "doc.md", "# Doc\n\nbody\n");
    write(&input, ".agents/memory/note.md", "# Note\n\nbody\n");

    for memory in [None, Some(input.join(".agents/memory"))] {
        let opts = CompileOptions {
            respect_ignore: false,
            memory,
            ..Default::default()
        };
        let result = compile(&input, &output, &opts).unwrap();
        assert_eq!(result.pages_total, 2);
        let entries = index_entries(&output);
        let notes: Vec<_> = entries.iter().filter(|e| e["title"] == "Note").collect();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0]["memory"], true);
        assert_eq!(notes[0]["path"], ".agents/memory/note.md");
    }
}

#[test]
fn memory_note_titled_like_a_project_doc_gets_its_own_id() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    write(&input, "docs/s.md", "---\ntitle: Setup\n---\n\nProject.\n");
    write(&input, "docs/l.md", "# Linker\n\nSee [[Setup]].\n");
    write(
        &input,
        ".agents/memory/s.md",
        "---\ntitle: Setup\n---\n\nNote.\n",
    );

    let result = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert_eq!(result.pages_total, 3);
    let entries = index_entries(&output);
    let project = entries.iter().find(|e| e["path"] == "docs/s.md").unwrap();
    assert_eq!(project["id"], "setup");
    assert_eq!(project["title"], "Setup");
    let note = entries
        .iter()
        .find(|e| e["path"] == ".agents/memory/s.md")
        .unwrap();
    assert_ne!(note["id"], "setup");
    assert!(note["title"].as_str().unwrap().contains("Setup"));
    let pages = wiki::lint::load_compiled_pages(&output).unwrap();
    assert_eq!(
        wiki::lint::lint(&pages).broken_links,
        [] as [(String, String); 0]
    );
    let linker = fs::read_to_string(output.join("linker.md")).unwrap();
    assert!(linker.contains("[[setup|Setup]]"), "{linker}");
}

#[test]
fn external_note_colliding_with_another_never_gets_the_key_folder_in_its_title() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    let external = dir.path().join("0123456789abcdef0123456789abcdef01234567");
    write(&input, "doc.md", "# Doc\n\nbody\n");
    write(&external, "a/n.md", "---\ntitle: Dup\n---\n\nA.\n");
    write(&external, "b/n.md", "---\ntitle: Dup\n---\n\nB.\n");
    let opts = CompileOptions {
        memory: Some(external),
        ..Default::default()
    };
    compile(&input, &output, &opts).unwrap();
    let entries = index_entries(&output);
    let dups: Vec<_> = entries
        .iter()
        .filter(|e| e["title"].as_str().unwrap().contains("Dup"))
        .collect();
    assert_eq!(dups.len(), 2);
    for e in dups {
        let t = e["title"].as_str().unwrap().to_lowercase();
        assert!(!t.contains("0123456789abcdef"), "{t}");
        assert!(
            e["path"].as_str().unwrap().starts_with('/')
                || e["path"].as_str().unwrap().contains(':')
        );
    }
}

#[test]
fn memory_files_join_the_watch_set() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let input = root.join("raw");
    let output = root.join("out");
    write(&input, "doc.md", "# Doc\n\nbody\n");
    write(&input, ".agents/memory/note.md", "# Note\n\nbody\n");
    let result = compile(&input, &output, &CompileOptions::default()).unwrap();
    assert!(result
        .watch_set
        .relevant(&input.join(".agents/memory/note.md")));
    assert!(result
        .watch_set
        .relevant(&input.join(".agents/memory/new.md")));
}

/// A title whose slug exceeds the file-name limit (255 bytes on common file
/// systems) still compiles: its id is capped and stays reachable.
#[test]
fn a_very_long_title_compiles_to_a_short_reachable_id() {
    let long = "word ".repeat(60); // 300 chars
    let cyr = "слово ".repeat(60); // 660 bytes of UTF-8
    let (_d, w) = common::compiled_wiki(&[
        ("long.md", &format!("# {long}\n\nBody of the long page.\n")),
        ("twin_a.md", &format!("# {long} alpha\n\nTwin A.\n")),
        ("twin_b.md", &format!("# {long} beta\n\nTwin B.\n")),
        ("cyr.md", &format!("# {cyr}\n\nКириличне тіло.\n")),
    ]);
    let mut ids = BTreeSet::new();
    for (path, q) in [
        ("long.md", "word"),
        ("twin_a.md", "alpha"),
        ("twin_b.md", "beta"),
        ("cyr.md", "слово"),
    ] {
        let hit = w
            .search(q, None, 50)
            .into_iter()
            .find(|h| h.path == path)
            .unwrap_or_else(|| panic!("{path} not found by search"));
        assert!(hit.id.len() <= 200, "{path}: id is {} bytes", hit.id.len());
        assert!(w.page(&hit.id).is_some(), "{path}: page {} missing", hit.id);
        ids.insert(hit.id);
    }
    assert_eq!(ids.len(), 4, "capped ids must stay unique: {ids:?}");
}

#[test]
fn badge_h1_page_resolves_by_its_clean_title() {
    let (_d, w) = common::compiled_wiki(&[(
        "README.md",
        "# Node Version Manager [![build](https://x/b.svg)](https://x/ci)\n\nBody.\n",
    )]);
    assert_eq!(
        w.resolve("Node Version Manager").unwrap(),
        "node_version_manager"
    );
}
