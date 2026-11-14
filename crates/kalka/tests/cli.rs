//! The `kalka` binary's subcommands end to end through `assert_cmd`: compile, search, neighbors, lint, and generate.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use tempfile::tempdir;

mod common;

#[test]
fn compile_then_search_cli() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("alpha.txt"), "# Alpha\n\nAlpha mentions Beta.\n").unwrap();
    fs::write(input.join("beta.txt"), "# Beta\n\nBeta content.\n").unwrap();

    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), output.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("Compiled 2 pages"));

    Command::cargo_bin("kalka")
        .unwrap()
        .args(["search", "beta", "--dir", output.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("beta"));

    // An unknown --kind must fail, not silently search unfiltered: the score
    // depends on the kind filter, so a typo would change numbers quietly.
    Command::cargo_bin("kalka")
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
    Command::cargo_bin("kalka")
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
    let assert = Command::cargo_bin("kalka")
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

    Command::cargo_bin("kalka")
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
    Command::cargo_bin("kalka")
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
    Command::cargo_bin("kalka")
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
    Command::cargo_bin("kalka")
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
    Command::cargo_bin("kalka")
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

#[test]
fn neighbors_cli_prints_a_pack() {
    let (_dir, output) = common::seed_corpus(12);
    let entry = &common::index(&output)["entries"][0];
    let id = entry["id"].as_str().unwrap();
    let title = entry["title"].as_str().unwrap();

    // Unbudgeted, the target block is the full rendered page, headed
    // `# <Title>`.
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["neighbors", id, "--dir", output.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains(format!("# {title}\n")));

    Command::cargo_bin("kalka")
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
    let (_dir, _, output) = common::compiled(&[
        ("alpha.txt", "# Alpha\n\nAlpha mentions Beta.\n"),
        ("beta.txt", "# Beta\n\nBeta content.\n"),
    ]);

    // `## Notes` is rendered last (rewrite::render_page), so appending lands
    // the link inside the one human-owned section — a legitimate hand edit,
    // not a mangled page.
    let alpha = output.join("alpha.md");
    let mut page = fs::read_to_string(&alpha).unwrap();
    page.push_str("\n[[Ghost]]\n");
    fs::write(&alpha, page).unwrap();

    Command::cargo_bin("kalka")
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
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), output.to_str().unwrap()])
        .assert()
        .success();
    Command::cargo_bin("kalka")
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
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), output.to_str().unwrap()])
        .assert()
        .success();
    Command::cargo_bin("kalka")
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
    let (_dir, _, output) = common::compiled(&[
        ("guide.txt", "# Guide\n\nThe zebra lives in prose.\n"),
        ("zoo.rs", "//! zebra module\npub fn zebra() {}\n"),
    ]);
    let search = |kind: &str| {
        Command::cargo_bin("kalka")
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
        .stderr(contains("unknown kind \"pdf\""))
        .stderr(contains(kalka::model::KindFilter::EXPECTED));
}

#[test]
fn search_cli_tag_lists_and_filters() {
    let (_dir, _, output) = common::compiled(&[
        ("a.md", "---\ntags: [mem]\n---\n# A\n\nzebra\n"),
        ("b.md", "# B\n\nzebra\n"),
    ]);
    let search = |args: &[&str]| {
        Command::cargo_bin("kalka")
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
        .stderr(contains("search needs a query, --tag or --docs"));
    search(&["--tag", "#"])
        .failure()
        .stderr(contains("empty tag"));
}

#[test]
fn cli_search_prints_the_same_lines_as_mcp() {
    let (_dir, _, output) = common::compiled(&[("lib.rs", "/// Adds.\npub fn add() {}\n")]);
    let run = |extra: &[&str]| {
        let mut args = vec!["search", "add", "--dir", output.to_str().unwrap()];
        args.extend(extra);
        let o = Command::cargo_bin("kalka")
            .unwrap()
            .args(args)
            .assert()
            .success();
        String::from_utf8(o.get_output().stdout.clone()).unwrap()
    };
    let w = kalka::query::Wiki::load(&output).unwrap();
    assert_eq!(
        run(&[]),
        kalka::serve::search_lines(&w.search("add", None, 10))
    );
    let json = run(&["--format", "json"]);
    assert!(serde_json::from_str::<serde_json::Value>(&json)
        .unwrap()
        .is_array());
}

#[test]
fn cli_page_names_a_missing_page_file() {
    let (_d, _raw, out) = common::compiled(&[
        ("docs/a.md", "# Alpha\n\nAlpha doc.\n"),
        ("src/g.rs", "/// Gamma fn.\npub fn gamma() {}\n"),
    ]);
    fs::remove_file(out.join("alpha.md")).unwrap();
    fs::remove_file(out.join("g.md")).unwrap();
    let a3 = "page \"alpha\" is in the index but its file alpha.md is missing from the wiki folder — recompile the wiki";
    let dir = out.to_str().unwrap();
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["page", "alpha", "--full", "--dir", dir])
        .assert()
        .code(1)
        .stderr(contains(a3));
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["page", "nope", "--dir", dir])
        .assert()
        .failure()
        .stderr(contains("unknown page"));
}

#[test]
fn cli_page_prints_the_same_text_as_page_text() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(input.join("src")).unwrap();
    fs::write(input.join("src/lib.rs"), "/// Adds.\npub fn add() {}\n").unwrap();
    kalka::compile(&input, &output, &kalka::CompileOptions::default()).unwrap();
    let w = kalka::query::Wiki::load(&output).unwrap();
    let run = |args: &[&str]| {
        let o = Command::cargo_bin("kalka")
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
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["page", "nope", "--dir", output.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("unknown page"));
}

#[test]
fn page_names_a_skipped_folder() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(input.join("src")).unwrap();
    fs::write(input.join("src/lib.rs"), "pub fn add() {}\n").unwrap();
    kalka::compile(&input, &output, &kalka::CompileOptions::default()).unwrap();
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["page", "docs/superpowers/plans/x.md", "--dir"])
        .arg(&output)
        .assert()
        .failure()
        .stderr(contains("pass \"docs/superpowers/plans/x.md\" in search's"));
}

#[test]
fn compile_memory_flag_repeats_and_local_is_an_error() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let ext1 = dir.path().join("ext1");
    let ext2 = dir.path().join("ext2");
    fs::create_dir_all(&input).unwrap();
    fs::create_dir_all(&ext1).unwrap();
    fs::create_dir_all(&ext2).unwrap();
    fs::write(input.join("doc.md"), "# Doc\n\nbody\n").unwrap();
    fs::write(ext1.join("a.md"), "# Ext One\n\nbody\n").unwrap();
    fs::write(ext2.join("b.md"), "# Ext Two\n\nbody\n").unwrap();

    let out = dir.path().join("out");
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), out.to_str().unwrap()])
        .args(["--memory", ext1.to_str().unwrap()])
        .args(["--memory", ext2.to_str().unwrap()])
        .assert()
        .success();
    let titles: Vec<String> = common::index_entries(&out)
        .iter()
        .map(|e| e["title"].as_str().unwrap().to_string())
        .collect();
    assert!(titles.contains(&"Ext One".to_string()), "{titles:?}");
    assert!(titles.contains(&"Ext Two".to_string()), "{titles:?}");

    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), out.to_str().unwrap()])
        .args(["--memory", "local"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "--memory local is no longer supported; pass the folder as --memory DIR",
        ));
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
        Command::cargo_bin("kalka")
            .unwrap()
            .args(["compile", input.to_str().unwrap(), out.to_str().unwrap()])
            .arg("--memory")
            .arg(memory)
            .assert()
            .success();
        let entries = common::index_entries(&out);
        let e = entries.iter().find(|e| e["title"] == "Ext Note").unwrap();
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
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), out.to_str().unwrap()])
        .args(["--memory", dir.path().join("nope").to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicates::str::contains("no such memory folder"));
}

#[test]
fn compile_memory_none_skips_repo_memory() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    fs::create_dir_all(input.join(".agents/memory")).unwrap();
    fs::write(input.join(".agents/memory/n.md"), "# N\n\nquokkanote\n").unwrap();
    fs::write(input.join("doc.md"), "# Doc\n\nbody\n").unwrap();
    let out = dir.path().join("out");
    // `--no-ignore` lets the main walk reach `.agents`: the note must not
    // come back from there as a plain page either.
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), out.to_str().unwrap()])
        .args(["--memory", "none", "--no-ignore"])
        .assert()
        .success()
        .stderr(contains("no such memory folder").not());
    let entries = common::index_entries(&out);
    assert!(entries.iter().any(|e| e["path"] == "doc.md"), "{entries:?}");
    assert!(
        !entries
            .iter()
            .any(|e| e["memory"] == true || e["path"].as_str().unwrap_or("").contains(".agents")),
        "{entries:?}"
    );
}

#[test]
fn compile_memory_none_with_folder_is_usage_error() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile", dir.path().to_str().unwrap()])
        .arg(dir.path().join("out"))
        .args(["--memory", "none", "--memory", dir.path().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(contains("--memory none cannot be combined with a folder"));
}

#[test]
fn serve_memory_requires_source() {
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["serve", "--memory", "none"])
        .assert()
        .failure();
}

#[test]
fn serve_memory_none_with_a_dir_is_an_error() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["serve", "--source", dir.path().to_str().unwrap()])
        .args(["--memory", "none", "--memory", dir.path().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "--memory none cannot be combined with a folder",
        ));
}

#[test]
fn cli_page_on_a_memory_note_prints_its_text_but_a_doc_prints_headings() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(input.join(".agents/memory")).unwrap();
    let body = "# Heading\n\nIntro.\n\nThe port is zorblax4471.\n";
    fs::write(
        input.join(".agents/memory/note.md"),
        format!("---\ntitle: Note\n---\n\n{body}"),
    )
    .unwrap();
    fs::write(input.join("doc.md"), body).unwrap();
    kalka::compile(&input, &output, &kalka::CompileOptions::default()).unwrap();
    let page = |id: &str| {
        let o = Command::cargo_bin("kalka")
            .unwrap()
            .args(["page", id, "--dir", output.to_str().unwrap()])
            .assert()
            .success();
        String::from_utf8(o.get_output().stdout.clone()).unwrap()
    };
    let note = common::index_entries(&output)
        .into_iter()
        .find(|e| e["memory"] == true)
        .unwrap();
    assert!(page(note["id"].as_str().unwrap()).contains("zorblax4471"));
    let doc = page("./doc.md");
    assert!(
        doc.contains("Heading") && !doc.contains("zorblax4471"),
        "{doc}"
    );
}

#[test]
fn cli_page_names_a_missing_memory_note_file() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(input.join(".agents/memory")).unwrap();
    fs::write(
        input.join(".agents/memory/note.md"),
        "---\ntitle: Note\n---\n\nThe port is zorblax4471.\n",
    )
    .unwrap();
    kalka::compile(&input, &output, &kalka::CompileOptions::default()).unwrap();
    let note = common::index_entries(&output)
        .into_iter()
        .find(|e| e["memory"] == true)
        .unwrap();
    let id = note["id"].as_str().unwrap();
    fs::remove_file(output.join(format!("{id}.md"))).unwrap();
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["page", id, "--dir", output.to_str().unwrap()])
        .assert()
        .code(1)
        .stderr(contains(format!(
            "page {id:?} is in the index but its file {id}.md is missing from the wiki folder — recompile the wiki"
        )));
}

#[test]
fn compile_reports_what_it_skipped_after_the_lint_line() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = input.join(".kulman");
    fs::create_dir_all(input.join("vendor/x")).unwrap();
    fs::write(input.join("a.txt"), "# A\n").unwrap();
    fs::write(input.join("vendor/x/b.txt"), "# B\n").unwrap();
    fs::write(input.join("vendor/c.txt"), "# C\n").unwrap();
    fs::write(input.join(".hidden.txt"), "# H\n").unwrap();

    let run = |extra: &[&str]| {
        Command::cargo_bin("kalka")
            .unwrap()
            .args(["compile", input.to_str().unwrap(), output.to_str().unwrap()])
            .args(extra)
            .assert()
            .success()
    };
    // Twice: the second run has an output folder that must stay uncounted.
    run(&[]);
    let out = run(&[]).get_output().stdout.clone();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(out.lines().nth(2), Some("Skipped: vendor 2, ignored 1"));
    assert_eq!(out.lines().count(), 3, "{out}");

    run(&["--no-ignore"]).stdout(contains("Skipped").not());
}

/// A query command run from `cwd` with no `--dir`.
fn search_from(cwd: &std::path::Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("kalka")
        .unwrap()
        .current_dir(cwd)
        .args(["search", "alpha"])
        .assert()
}

#[test]
fn the_default_wiki_is_dot_kulman() {
    let dir = tempdir().unwrap();
    let cwd = dir.path();
    fs::write(cwd.join("alpha.txt"), "# Alpha\n\nAlpha body.\n").unwrap();
    let compile = |output: &str| {
        Command::cargo_bin("kalka")
            .unwrap()
            .current_dir(cwd)
            .args(["compile", ".", output])
            .assert()
            .success();
    };
    compile(".kulman");
    search_from(cwd).success().stdout(contains("alpha"));

    // An old `.wiki/` is not the default: with `out` beside it, `out` is read.
    fs::remove_dir_all(cwd.join(".kulman")).unwrap();
    fs::create_dir(cwd.join(".wiki")).unwrap();
    compile("out");
    search_from(cwd).success().stdout(contains("alpha"));
}

#[test]
fn read_commands_default_to_dot_wiki_then_out() {
    let dir = tempdir().unwrap();
    let cwd = dir.path();
    fs::write(cwd.join("alpha.txt"), "# Alpha\n\nAlpha body.\n").unwrap();
    search_from(cwd).failure().stderr(contains("load wiki"));

    let compile = |output: &str| {
        Command::cargo_bin("kalka")
            .unwrap()
            .current_dir(cwd)
            .args(["compile", ".", output])
            .assert()
            .success();
    };
    compile("out");
    search_from(cwd).success().stdout(contains("alpha"));

    // `.kulman` wins over `out`: empty it would fail the load, not fall through.
    fs::create_dir(cwd.join(".kulman")).unwrap();
    search_from(cwd).failure().stderr(contains("load wiki"));
    compile(".kulman");
    search_from(cwd).success().stdout(contains("alpha"));

    // An explicit `--dir` wins over both.
    Command::cargo_bin("kalka")
        .unwrap()
        .current_dir(cwd)
        .args(["search", "alpha", "--dir", "nowhere"])
        .assert()
        .failure()
        .stderr(contains("load wiki"));

    // `serve` with neither `--dir` nor `--source` takes the same default.
    fs::remove_dir_all(cwd.join("out")).unwrap();
    let serve = |cwd: &std::path::Path| {
        Command::cargo_bin("kalka")
            .unwrap()
            .current_dir(cwd)
            .arg("serve")
            .write_stdin("")
            .assert()
            .failure()
    };
    // The wiki loaded, then stdin closed before the handshake.
    serve(cwd).stderr(contains("connection closed"));
    serve(tempdir().unwrap().path()).stderr(contains("compile"));

    for cmd in ["search", "page", "neighbors", "callers", "lint", "serve"] {
        Command::cargo_bin("kalka")
            .unwrap()
            .args([cmd, "--help"])
            .assert()
            .success()
            .stdout(contains("`.kulman` if that directory exists, else `out`"));
    }
}

#[test]
fn cli_search_docs_empty_query_lists_labels() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("plan.md"),
        "# Plan\n\n## Task 2\n\n**T2d. Remaining matrix.**\n\n**Files:**\n- a.rs\n- **Mutants:** x\n\n**T2e (QA-H T5): next.**\nbody\n```sh\n## not a heading\n**T9. not a label**\n```\n## Task 3\n- **D-d, swap (H1).** ruling\n",
    )
    .unwrap();
    // No compiled wiki in the directory: `--docs` does not need one.
    let out = Command::cargo_bin("kalka")
        .unwrap()
        .current_dir(dir.path())
        .args(["search", "--docs", "plan.md"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    let lines: Vec<_> = out.lines().collect();
    assert_eq!(lines.len(), 7, "{out}");
    assert_eq!(lines[..2], ["plan.md", "  1-18  Plan"]);
}

#[test]
fn neighbors_symbol_walks_through_a_pointer_returning_definition() {
    // The walk from `cost` must step into the pointer-returning
    // `Router::pick_venue` definition and on to `in_maintenance`; with no
    // owner on that definition it stops at the header declaration.
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(input.join("src")).unwrap();
    fs::write(
        input.join("src/router.h"),
        "class Router {\npublic:\n  const Venue* pick_venue(int id);\n  int cost(int id);\n  bool in_maintenance(int id);\n};\n",
    )
    .unwrap();
    fs::write(
        input.join("src/router.cpp"),
        "const Venue* Router::pick_venue(int id) { in_maintenance(id); return nullptr; }\nint Router::cost(int id) { pick_venue(id); return 0; }\nbool Router::in_maintenance(int id) { return id == 0; }\n",
    )
    .unwrap();
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile", input.to_str().unwrap(), output.to_str().unwrap()])
        .assert()
        .success();
    Command::cargo_bin("kalka")
        .unwrap()
        .args([
            "neighbors",
            "src/router.cpp",
            "--symbol",
            "cost",
            "--dir",
            output.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(contains("in_maintenance"));
}

#[test]
fn cli_search_says_which_names_it_did_not_find() {
    let (_dir, _, output) = common::compiled(&[("a.py", "def _ack():\n    pass\n")]);
    let o = Command::cargo_bin("kalka")
        .unwrap()
        .args(["search", "_ack _nope", "--dir", output.to_str().unwrap()])
        .assert()
        .success();
    let out = String::from_utf8(o.get_output().stdout.clone()).unwrap();
    assert!(out.contains("a.py:1-2  _ack"), "{out}");
    assert!(out.ends_with("not found: _nope\n"), "{out}");
}

#[test]
fn compile_prints_stage_timings_only_with_wiki_timings() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("alpha.txt"), "# Alpha\n\nAlpha mentions Beta.\n").unwrap();
    fs::write(input.join("beta.txt"), "# Beta\n\nBeta content.\n").unwrap();

    let run = |name: &str, env: Option<&str>| {
        let out = dir.path().join(name);
        let mut cmd = Command::cargo_bin("kalka").unwrap();
        cmd.args(["compile", input.to_str().unwrap(), out.to_str().unwrap()]);
        if let Some(v) = env {
            cmd.env("WIKI_TIMINGS", v);
        }
        let o = cmd.assert().success().get_output().clone();
        (out, String::from_utf8(o.stderr).unwrap())
    };
    let (out1, err1) = run("out1", Some("1"));
    let (out2, err2) = run("out2", None);
    let (out3, err3) = run("out3", Some("0"));

    let lines: Vec<&str> = err1
        .lines()
        .filter(|l| l.starts_with("timings: "))
        .collect();
    assert_eq!(lines.len(), 1, "stderr: {err1}");
    let pairs: Vec<(&str, &str)> = lines[0]["timings: ".len()..]
        .split(' ')
        .map(|kv| kv.split_once('=').unwrap())
        .collect();
    let keys: Vec<&str> = pairs.iter().map(|p| p.0).collect();
    assert_eq!(
        keys,
        ["total", "walk", "extract", "global", "render", "write", "tail", "files", "written"]
    );
    let micros = |s: &str| -> u64 {
        let ms = s.strip_suffix("ms").unwrap_or_else(|| panic!("no ms: {s}"));
        let (w, f) = ms.split_once('.').unwrap();
        assert_eq!(f.len(), 3, "{s}");
        w.parse::<u64>().unwrap() * 1000 + f.parse::<u64>().unwrap()
    };
    let total = micros(pairs[0].1);
    let stages: u64 = pairs[1..7].iter().map(|p| micros(p.1)).sum();
    assert_eq!(stages, total);
    assert_eq!(
        (
            pairs[7].1.parse::<usize>().unwrap(),
            pairs[8].1.parse::<usize>().unwrap()
        ),
        (2, 2)
    );
    assert!(!err2.contains("timings:"), "{err2}");
    assert!(!err3.contains("timings:"), "{err3}");

    let snapshot = |root: &std::path::Path| {
        let mut v = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(root).unwrap().to_path_buf();
                    v.push((rel, fs::read(&p).unwrap()));
                }
            }
        }
        v.sort();
        v
    };
    let s1 = snapshot(&out1);
    assert_eq!(s1, snapshot(&out2));
    assert_eq!(s1, snapshot(&out3));
}

#[test]
fn the_binary_is_called_kalka() {
    Command::cargo_bin("kalka")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage: kalka"));
}
