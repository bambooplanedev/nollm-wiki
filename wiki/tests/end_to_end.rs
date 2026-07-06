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
