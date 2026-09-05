//! The `wiki` binary's subcommands end to end through `assert_cmd`: compile, search, neighbors, lint, generate, and `--watch`.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
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

    // An unknown --kind must fail, not silently search unfiltered: the score
    // depends on the kind filter, so a typo would change numbers quietly.
    Command::cargo_bin("wiki")
        .unwrap()
        .args([
            "search",
            "beta",
            "--dir",
            output.to_str().unwrap(),
            "--kind",
            "bogus",
        ])
        .assert()
        .failure()
        .stderr(contains("unknown kind \"bogus\""));
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

/// seed-42 corpus compiled into `<dir>/out`; returns the tempdir.
fn compile_seed_corpus() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    wiki::generator::generate_corpus(&input, 12, 42).unwrap();
    wiki::compile(&input, &output, &wiki::CompileOptions::default()).unwrap();
    dir
}

#[test]
fn neighbors_cli_prints_a_pack() {
    let dir = compile_seed_corpus();
    let output = dir.path().join("out");
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.join("index.json")).unwrap()).unwrap();
    let entry = &index["entries"][0];
    let id = entry["id"].as_str().unwrap();
    let title = entry["title"].as_str().unwrap();

    // Unbudgeted, the target block is the full rendered page, headed
    // `# <Title>`.
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["neighbors", id, "--dir", output.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains(format!("# {title}\n")));

    Command::cargo_bin("wiki")
        .unwrap()
        .args([
            "neighbors",
            "no_such_page",
            "--dir",
            output.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(contains("unknown id"));
}

#[test]
fn lint_cli_reports_a_real_broken_link() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("alpha.txt"), "# Alpha\n\nAlpha mentions Beta.\n").unwrap();
    fs::write(input.join("beta.txt"), "# Beta\n\nBeta content.\n").unwrap();
    wiki::compile(&input, &output, &wiki::CompileOptions::default()).unwrap();

    // `## Notes` is rendered last (rewrite::render_page), so appending lands
    // the link inside the one human-owned section — a legitimate hand edit,
    // not a mangled page.
    let alpha = output.join("alpha.md");
    let mut page = fs::read_to_string(&alpha).unwrap();
    page.push_str("\n[[Ghost]]\n");
    fs::write(&alpha, page).unwrap();

    Command::cargo_bin("wiki")
        .unwrap()
        .args(["lint", "--dir", output.to_str().unwrap()])
        .assert()
        // Broken links fail the process so lint can gate a build; orphans
        // alone (see `lint_cli_exits_zero_with_only_orphans`) do not.
        .code(1)
        .stdout(contains("Linted 2 pages: 1 broken links"))
        .stdout(contains("  broken: alpha -> \"Ghost\"\n"));
}

#[test]
fn lint_cli_exits_zero_with_only_orphans() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("alpha.txt"), "# Alpha\n\nNobody links here.\n").unwrap();
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), output.to_str().unwrap()])
        .assert()
        .success();
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["lint", "--dir", output.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(contains("0 broken links, 1 orphans"))
        .stdout(contains("  orphan: alpha\n"));
}

#[test]
fn search_cli_accepts_valid_kinds() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("guide.txt"),
        "# Guide\n\nThe zebra lives in prose.\n",
    )
    .unwrap();
    fs::write(
        input.join("zoo.rs"),
        "//! zebra module\npub fn zebra() {}\n",
    )
    .unwrap();
    wiki::compile(&input, &output, &wiki::CompileOptions::default()).unwrap();
    let search = |kind: &str| {
        Command::cargo_bin("wiki")
            .unwrap()
            .args([
                "search",
                "zebra",
                "--dir",
                output.to_str().unwrap(),
                "--kind",
                kind,
            ])
            .assert()
    };

    search("text")
        .success()
        .stdout(contains("guide\t"))
        .stdout(contains("zoo\t").not());
    search("code:rust")
        .success()
        .stdout(contains("zoo\t"))
        .stdout(contains("guide\t").not());
    search("pdf")
        .failure()
        .stderr(contains("expected text, markdown, or code:<lang>"));
}

/// Kills the watcher when the test ends — including on a failed assertion,
/// so a stray `wiki compile --watch` never outlives its tempdir.
struct KillOnDrop(std::process::Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(path: &std::path::Path, what: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out after 10s waiting for {what}: {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn watch_recompiles_on_change() {
    use std::io::BufRead;
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("alpha.txt"), "# Alpha\n\nAlpha stands alone.\n").unwrap();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wiki"))
        .args([
            "compile",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--watch",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = std::io::BufReader::new(child.stderr.take().unwrap());
    let _guard = KillOnDrop(child);

    // The watcher is registered only after the initial compile and before
    // `watching …` is printed (watch.rs); a change written before that line
    // would be missed, so wait for it rather than for index.json alone.
    let mut line = String::new();
    loop {
        line.clear();
        let n = stderr.read_line(&mut line).unwrap();
        assert!(n > 0, "watcher exited before printing `watching`");
        if line.starts_with("watching ") {
            break;
        }
    }
    assert!(
        output.join("index.json").exists(),
        "initial compile did not write index.json"
    );
    assert!(!output.join("gamma.md").exists());

    fs::write(input.join("gamma.txt"), "# Gamma\n\nGamma arrived later.\n").unwrap();
    wait_for(
        &output.join("gamma.md"),
        "the watcher to recompile the new gamma.txt into gamma.md",
    );
}
