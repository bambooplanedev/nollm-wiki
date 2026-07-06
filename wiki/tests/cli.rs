use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use tempfile::tempdir;

#[test]
fn compile_then_search_cli() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("alpha.txt"), "# Alpha\n\nAlpha mentions Beta.\n").unwrap();
    fs::write(input.join("beta.txt"), "# Beta\n\nBeta content.\n").unwrap();

    Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), output.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("Compiled 2 pages"));

    Command::cargo_bin("wiki")
        .unwrap()
        .args(["search", "beta", "--dir", output.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("beta"));
}

#[test]
fn generate_cli_creates_files() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("wiki")
        .unwrap()
        .args([
            "generate",
            dir.path().to_str().unwrap(),
            "--files",
            "5",
            "--seed",
            "1",
        ])
        .assert()
        .success();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 5);
}
