use std::fs;
use tempfile::tempdir;
use wiki::query::{PackBudget, Wiki};
use wiki::{compile, CompileOptions};

fn build() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("alpha.txt"),
        "# Alpha\n\nAlpha mentions Beta and Gamma.\n",
    )
    .unwrap();
    fs::write(
        input.join("beta.txt"),
        "# Beta\n\nBeta is about beta things.\n",
    )
    .unwrap();
    fs::write(input.join("gamma.txt"), "# Gamma\n\nGamma stands alone.\n").unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    // stash output path by leaking dir via returning it
    dir
}

#[test]
fn search_finds_by_name_and_body() {
    let dir = build();
    let w = Wiki::load(&dir.path().join("out")).unwrap();
    let hits = w.search("beta", None, 10);
    assert!(hits.iter().any(|h| h.id == "beta"));
}

#[test]
fn neighbors_pack_includes_target_first_and_respects_max_nodes() {
    let dir = build();
    let w = Wiki::load(&dir.path().join("out")).unwrap();
    let budget = PackBudget {
        max_nodes: Some(2),
        ..Default::default()
    };
    let pack = w.neighbors("alpha", 1, &budget).unwrap();
    assert_eq!(pack.included.first().map(String::as_str), Some("alpha"));
    assert!(pack.included.len() <= 2);
    assert!(pack.text.contains("# Alpha"));
}
