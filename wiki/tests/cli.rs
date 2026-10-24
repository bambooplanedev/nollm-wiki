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
fn stale_page_from_id_scheme_change_is_pruned_once_its_notes_migrate() {
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
    let alpha_one = output.join("alpha_one.md");
    assert!(alpha_one.exists());

    // Hand-write real notes on alpha_one.md before the rename.
    let text = fs::read_to_string(&alpha_one).unwrap();
    let replaced = text.replace(
        "_(add your own notes here — preserved on recompile)_",
        "HAND WRITTEN NOTE",
    );
    assert_ne!(text, replaced, "placeholder not found in alpha_one.md");
    fs::write(&alpha_one, replaced).unwrap();

    // Rename the title so its slug changes (alpha_one -> alpha_two). The
    // source file on disk (note.md) is unchanged apart from its title, so
    // `note.md`'s notes follow it to alpha_two.md regardless of id.
    fs::write(&renamed, "---\ntitle: Alpha Two\n---\n\nAlpha body.\n").unwrap();

    // Compile #2: non-incremental. Prune now runs unconditionally, but
    // alpha_one.md's notes were migrated onto alpha_two.md (same source
    // file), so alpha_one.md is `consumed` and safe to delete outright —
    // no warning, because nothing hand-written was left behind.
    let assert = Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), output.to_str().unwrap()])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        !stderr.contains("previous id scheme"),
        "migrated notes must not trigger the leftover-notes warning: {stderr}"
    );

    assert!(
        !alpha_one.exists(),
        "alpha_one.md's notes were migrated onto alpha_two.md, so it must be pruned"
    );
    let alpha_two = fs::read_to_string(output.join("alpha_two.md")).unwrap();
    assert!(
        alpha_two.contains("HAND WRITTEN NOTE"),
        "the hand-written note must have migrated to alpha_two.md: {alpha_two}"
    );
}

/// Without `--project` the name comes from the input path, so `compile .`
/// and `compile "$PWD"` disagree; the flag pins it.
#[test]
fn compile_project_flag_names_the_manifest() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("alpha.txt"), "# Alpha\n\nAlpha.\n").unwrap();

    Command::cargo_bin("wiki")
        .unwrap()
        .args([
            "compile",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--project",
            "demo",
        ])
        .assert()
        .success();

    let index = fs::read_to_string(output.join("index.json")).unwrap();
    assert!(index.contains("\"project\": \"demo\""), "{index}");
    assert!(fs::read_to_string(output.join("llms.txt"))
        .unwrap()
        .starts_with("# demo\n"));

    // --verbose was a documented no-op; it is gone, and clap says so.
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["--verbose", "lint", "--dir", output.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("unexpected argument '--verbose'"));
}

/// `root` is the input directory's own name even for `compile .`, where
/// `project` falls back to `wiki`; `page` matches absolute paths against it.
#[test]
fn compile_dot_records_the_directory_name_as_root() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    fs::create_dir_all(input.join("src")).unwrap();
    fs::write(input.join("src/alpha.rs"), "pub fn alpha() {}\n").unwrap();
    Command::cargo_bin("wiki")
        .unwrap()
        .current_dir(&input)
        .args(["compile", ".", "out"])
        .assert()
        .success();

    let index = fs::read_to_string(input.join("out/index.json")).unwrap();
    assert!(
        index.contains("\"project\": \"wiki\",\n  \"root\": \"raw\","),
        "{index}"
    );
    let abs = input.join("src/alpha.rs");
    Command::cargo_bin("wiki")
        .unwrap()
        .current_dir(&input)
        .args(["page", abs.to_str().unwrap(), "--dir", "out"])
        .assert()
        .success()
        .stdout(contains("src/alpha.rs  code:rust"));
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
        .stdout(contains("0 broken links, 1 orphans, 0 large"))
        .stdout(contains("  orphan: alpha\n"));
}

#[test]
fn lint_cli_reports_a_page_too_large_for_a_neighbors_pack() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("big.md"),
        format!("# Big\n\n{}\n", "word ".repeat(4000)),
    )
    .unwrap();
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
        .stdout(contains(
            "Linted 1 pages: 0 broken links, 1 orphans, 1 large",
        ))
        .stdout(contains("  large: big ("))
        .stdout(contains(" tokens)\n"));
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
        .stdout(contains("guide  "))
        .stdout(contains("zoo  ").not());
    search("code:rust")
        .success()
        .stdout(contains("zoo  "))
        .stdout(contains("guide  ").not());
    search("pdf")
        .failure()
        .stderr(contains(wiki::model::KindFilter::EXPECTED));
}

#[test]
fn search_cli_tag_lists_and_filters() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("a.md"), "---\ntags: [mem]\n---\n# A\n\nzebra\n").unwrap();
    fs::write(input.join("b.md"), "# B\n\nzebra\n").unwrap();
    wiki::compile(&input, &output, &wiki::CompileOptions::default()).unwrap();
    let search = |args: &[&str]| {
        Command::cargo_bin("wiki")
            .unwrap()
            .arg("search")
            .args(args)
            .args(["--dir", output.to_str().unwrap()])
            .assert()
    };

    search(&["--tag", "mem"])
        .success()
        .stdout(contains("a  "))
        .stdout(contains("b  ").not());
    search(&["zebra", "--tag", "#MEM"])
        .success()
        .stdout(contains("a  "))
        .stdout(contains("b  ").not());
    search(&[])
        .failure()
        .stderr(contains("search needs a query or --tag"));
    search(&["--tag", "#"])
        .failure()
        .stderr(contains("empty tag"));
}

#[test]
fn unknown_kind_lists_groups() {
    let dir = compile_seed_corpus();
    let out = dir.path().join("out");
    Command::cargo_bin("wiki")
        .unwrap()
        .args([
            "search",
            "x",
            "--dir",
            out.to_str().unwrap(),
            "--kind",
            "nope",
        ])
        .assert()
        .failure()
        .stderr(contains("code, docs, text, markdown, or code:<lang>"));
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

#[test]
fn cli_search_prints_the_same_lines_as_mcp() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("lib.rs"), "/// Adds.\npub fn add() {}\n").unwrap();
    wiki::compile(&input, &output, &wiki::CompileOptions::default()).unwrap();
    let run = |extra: &[&str]| {
        let mut args = vec!["search", "add", "--dir", output.to_str().unwrap()];
        args.extend(extra);
        let o = Command::cargo_bin("wiki")
            .unwrap()
            .args(args)
            .assert()
            .success();
        String::from_utf8(o.get_output().stdout.clone()).unwrap()
    };
    let w = wiki::query::Wiki::load(&output).unwrap();
    assert_eq!(
        run(&[]),
        wiki::serve::search_lines(&w.search("add", None, 10))
    );
    let json = run(&["--format", "json"]);
    assert!(serde_json::from_str::<serde_json::Value>(&json)
        .unwrap()
        .is_array());
}

#[test]
fn cli_page_prints_the_same_text_as_page_text() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(input.join("src")).unwrap();
    fs::write(input.join("src/lib.rs"), "/// Adds.\npub fn add() {}\n").unwrap();
    wiki::compile(&input, &output, &wiki::CompileOptions::default()).unwrap();
    let w = wiki::query::Wiki::load(&output).unwrap();
    let run = |args: &[&str]| {
        let o = Command::cargo_bin("wiki")
            .unwrap()
            .arg("page")
            .args(args)
            .args(["--dir", output.to_str().unwrap()])
            .assert()
            .success();
        String::from_utf8(o.get_output().stdout.clone()).unwrap()
    };
    assert_eq!(run(&["lib"]), w.page_text("lib", false, None).unwrap());
    assert_eq!(
        run(&["./src/lib.rs", "--full"]),
        w.page_text("lib", true, None).unwrap()
    );
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["page", "nope", "--dir", output.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("unknown page"));
}

#[test]
fn compile_memory_flag_takes_a_dir_or_local() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let ext = dir.path().join("ext");
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    let lad = dir.path().join("lad");
    fs::create_dir_all(&input).unwrap();
    fs::create_dir_all(&ext).unwrap();
    fs::write(input.join("doc.md"), "# Doc\n\nbody\n").unwrap();
    fs::write(ext.join("n.md"), "# Ext Note\n\nbody\n").unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&input)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "git {args:?}");
    };
    let have_git = std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok();

    let wiki = |out: &std::path::Path, memory: &str| {
        Command::cargo_bin("wiki")
            .unwrap()
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &xdg)
            .env("LOCALAPPDATA", &lad)
            .args([
                "compile",
                input.to_str().unwrap(),
                out.to_str().unwrap(),
                "--memory",
                memory,
            ])
            .assert()
            .success();
    };
    let titles = |out: &std::path::Path| -> Vec<String> {
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(out.join("index.json")).unwrap()).unwrap();
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["title"].as_str().unwrap().to_string())
            .collect()
    };

    let out1 = dir.path().join("out1");
    wiki(&out1, ext.to_str().unwrap());
    assert!(titles(&out1).contains(&"Ext Note".to_string()));

    if !have_git {
        return;
    }
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "one"]);
    let env = |k: &str| match k {
        "HOME" => Some(home.to_string_lossy().to_string()),
        "XDG_DATA_HOME" => Some(xdg.to_string_lossy().to_string()),
        "LOCALAPPDATA" => Some(lad.to_string_lossy().to_string()),
        _ => None,
    };
    let key = wiki::memory::repo_key(&input).unwrap();
    let local = wiki::memory::data_dir_for(std::env::consts::OS, &env)
        .unwrap()
        .join("wiki/memory")
        .join(key);

    // An absent local folder is no memory, not an error.
    let out2 = dir.path().join("out2");
    wiki(&out2, "local");
    assert_eq!(titles(&out2), vec!["Doc".to_string()]);

    fs::create_dir_all(&local).unwrap();
    fs::write(local.join("l.md"), "# Local Note\n\nbody\n").unwrap();
    let out3 = dir.path().join("out3");
    wiki(&out3, "local");
    assert!(titles(&out3).contains(&"Local Note".to_string()));
}

#[test]
fn compile_memory_dir_with_dotdot_gives_the_same_page_path() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let ext = dir.path().join("ext");
    fs::create_dir_all(&input).unwrap();
    fs::create_dir_all(&ext).unwrap();
    fs::write(input.join("doc.md"), "# Doc\n\nbody\n").unwrap();
    fs::write(ext.join("n.md"), "# Ext Note\n\nbody\n").unwrap();
    let path_of = |out: &str, memory: &std::path::Path| -> String {
        let out = dir.path().join(out);
        Command::cargo_bin("wiki")
            .unwrap()
            .args(["compile", input.to_str().unwrap(), out.to_str().unwrap()])
            .arg("--memory")
            .arg(memory)
            .assert()
            .success();
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(out.join("index.json")).unwrap()).unwrap();
        let e = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["title"] == "Ext Note")
            .unwrap();
        e["path"].as_str().unwrap().to_string()
    };
    let plain = path_of("o1", &ext);
    let dotted = path_of("o2", &ext.join("../ext/../ext"));
    assert!(!plain.contains(".."));
    assert_eq!(plain, dotted);
}

#[test]
fn compile_memory_dir_that_does_not_exist_warns_and_continues() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("doc.md"), "# Doc\n\nbody\n").unwrap();
    let out = dir.path().join("out");
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), out.to_str().unwrap()])
        .args(["--memory", dir.path().join("nope").to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicates::str::contains("no such memory folder"));
}

#[test]
fn serve_memory_requires_source() {
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["serve", "--memory", "none"])
        .assert()
        .failure();
}
