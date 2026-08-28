use std::fs;
use tempfile::tempdir;
use wiki::{compile, CompileOptions};

mod common;
use common::{exports, imports};

fn write(root: &std::path::Path, name: &str, body: &str) {
    let p = root.join(name);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

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
