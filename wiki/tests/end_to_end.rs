use std::fs;
use tempfile::tempdir;
use wiki::{compile, CompileOptions};

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
    assert!(alpha.contains("[[Beta]]"));
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
