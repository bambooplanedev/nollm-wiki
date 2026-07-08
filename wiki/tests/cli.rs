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
fn stale_page_from_id_scheme_change_warns_but_does_not_delete() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    let renamed = input.join("note.md");
    fs::write(&renamed, "---\ntitle: Alpha One\n---\n\nAlpha body.\n").unwrap();
    fs::write(
        input.join("beta.md"),
        "---\ntitle: Beta\n---\n\nBeta body.\n",
    )
    .unwrap();

    // Compile #1: incremental. Writes alpha_one.md and records it in the cache.
    Command::cargo_bin("wiki")
        .unwrap()
        .args([
            "compile",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--incremental",
        ])
        .assert()
        .success();
    assert!(output.join("alpha_one.md").exists());

    // Rename the title so its slug changes (alpha_one -> alpha_two). The
    // source file on disk is unchanged apart from its content.
    fs::write(&renamed, "---\ntitle: Alpha Two\n---\n\nAlpha body.\n").unwrap();

    // Compile #2: non-incremental. It never prunes, and it doesn't overwrite
    // the cache, so `prior_ids` (from compile #1's cache) still contains
    // alpha_one while `live` now contains alpha_two — the migration warning
    // must fire on stderr, and alpha_one.md must be left on disk untouched.
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), output.to_str().unwrap()])
        .assert()
        .success()
        .stderr(contains("from a previous id scheme remain"))
        .stderr(contains("alpha_one.md"));

    assert!(
        output.join("alpha_one.md").exists(),
        "stale page must be warned about, not deleted"
    );
    assert!(output.join("alpha_two.md").exists());
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
