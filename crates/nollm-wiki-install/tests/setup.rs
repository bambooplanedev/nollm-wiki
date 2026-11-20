mod common;

use std::fs;
use std::path::PathBuf;

use common::{nollm_wiki_install, release, stderr};
use serde_json::{json, Value};

/// A module that appends `<name> <args>` to `$NOLLM_WIKI_TEST_LOG`.
const LOGS: &[u8] = b"#!/bin/sh\necho \"$(basename \"$0\") $*\" >> \"$NOLLM_WIKI_TEST_LOG\"\n";
/// The same, then exits 3.
const LOGS_THEN_FAILS: &[u8] =
    b"#!/bin/sh\necho \"$(basename \"$0\") $*\" >> \"$NOLLM_WIKI_TEST_LOG\"\nexit 3\n";

/// An empty log: no module ran.
const NONE: [&str; 0] = [];

/// A temp root with `release/` (wiki and plaz), `home` holding `modules`, and `project/`.
struct Fixture {
    t: tempfile::TempDir,
    base: String,
    home: PathBuf,
}

fn installed(wiki: &[u8], modules: &str) -> Fixture {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("wiki", wiki), ("plaz", LOGS)],
    );
    let home = t.path().join("home");
    nollm_wiki_install(&home, &base)
        .args(["install", "--modules", modules])
        .assert()
        .success();
    fs::create_dir(t.path().join("project")).unwrap();
    Fixture { t, base, home }
}

impl Fixture {
    fn log(&self) -> PathBuf {
        self.t.path().join("log")
    }

    /// `nollm-wiki-install setup <args>` in the project dir, stdin closed.
    fn setup(&self, args: &[&str]) -> std::process::Output {
        nollm_wiki_install(&self.home, &self.base)
            .arg("setup")
            .args(args)
            .current_dir(self.t.path().join("project"))
            .env("NOLLM_WIKI_TEST_LOG", self.log())
            .output()
            .unwrap()
    }

    fn logged(&self) -> Vec<String> {
        match fs::read_to_string(self.log()) {
            Ok(text) => text.lines().map(str::to_string).collect(),
            Err(_) => Vec::new(),
        }
    }
}

fn usage(out: &std::process::Output, expect: &str) {
    assert_eq!(out.status.code(), Some(2), "{}", stderr(out));
    assert!(stderr(out).contains(expect), "{}", stderr(out));
}

#[test]
fn setup_passes_the_choices_to_each_module() {
    let f = installed(LOGS, "wiki,plaz");
    let out = f.setup(&["--agent", "claude", "--scope", "repo", "--no-memory"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "wiki setup --agent claude --scope repo --no-memory",
            "plaz setup --scope repo"
        ]
    );
    let out = f.setup(&[
        "--modules",
        "wiki",
        "--scope",
        "local",
        "--no-hook",
        "--yes",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged()[2..],
        ["wiki setup --scope local --no-hook --memory"]
    );
    let out = f.setup(&["--modules", "wiki", "--scope", "repo", "--hook"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.logged()[3..], ["wiki setup --scope repo --hook --memory"]);
    usage(
        &f.setup(&["--scope", "repo", "--hook", "--no-hook"]),
        "cannot be used with",
    );
}

#[test]
fn setup_defaults_to_every_installed_module_with_memory() {
    let f = installed(LOGS, "wiki,plaz");
    let out = f.setup(&["--scope", "local"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "wiki setup --scope local --memory",
            "plaz setup --scope local"
        ]
    );
    // Only what is installed.
    let f = installed(LOGS, "plaz");
    let out = f.setup(&["--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.logged(), ["plaz setup --scope repo"]);
}

#[test]
fn setup_takes_the_memory_default_from_the_manifest() {
    let f = installed(LOGS, "wiki,plaz");
    let path = f.home.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["modules"][0]["options"] =
        json!([{"name": "memory", "summary": "agent memory", "default": false}]);
    fs::write(&path, manifest.to_string()).unwrap();
    let out = f.setup(&["--modules", "wiki", "--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = f.setup(&["--modules", "wiki", "--scope", "repo", "--memory"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "wiki setup --scope repo --no-memory",
            "wiki setup --scope repo --memory"
        ]
    );
}

#[test]
fn setup_without_scope_is_usage() {
    let f = installed(LOGS, "wiki,plaz");
    for args in [&[][..], &["--yes"], &["--modules", "plaz"]] {
        usage(
            &f.setup(args),
            "nollm-wiki-install: --scope is required (repo or local)",
        );
    }
    assert_eq!(f.logged(), NONE);
}

#[test]
fn setup_remove_without_scope_is_usage_when_wiki_is_chosen() {
    let f = installed(LOGS, "wiki,plaz");
    for args in [&["--remove"][..], &["--remove", "--modules", "wiki"]] {
        usage(
            &f.setup(args),
            "nollm-wiki-install: --scope is required (repo or local)",
        );
    }
    assert_eq!(f.logged(), NONE);
}

#[test]
fn setup_remove_of_plaz_alone_needs_no_scope() {
    let f = installed(LOGS, "wiki,plaz");
    let out = f.setup(&["--remove", "--modules", "plaz"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.logged(), ["plaz setup --remove"]);
}

#[test]
fn setup_remove_reaches_every_module() {
    let f = installed(LOGS, "wiki,plaz");
    let out = f.setup(&["--remove", "--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "wiki setup --scope repo --memory --remove",
            "plaz setup --scope repo --remove"
        ]
    );
}

#[test]
fn setup_of_a_module_not_installed_says_how() {
    let f = installed(LOGS, "plaz");
    let out = f.setup(&["--modules", "wiki", "--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect =
        "nollm-wiki-install: wiki is not installed; run nollm-wiki-install install --modules wiki";
    assert!(stderr(&out).contains(expect), "{}", stderr(&out));
    usage(
        &f.setup(&["--modules", "nollm-wiki-install", "--scope", "repo"]),
        "nollm-wiki-install: nollm-wiki-install is not a module",
    );
    // Not in the release either: no install hint, which would fail.
    let out = f.setup(&["--modules", "nope", "--scope", "repo"]);
    usage(
        &out,
        "nollm-wiki-install: --modules: unknown module \"nope\" (known: wiki, plaz)",
    );
    assert!(
        !stderr(&out).contains("install --modules"),
        "{}",
        stderr(&out)
    );
    usage(
        &f.setup(&["--modules", "plaz,", "--scope", "repo"]),
        "nollm-wiki-install: --modules: bad name \"\"",
    );
    assert_eq!(f.logged(), NONE);
    let out = f.setup(&["--modules", " plaz ", "--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.logged(), ["plaz setup --scope repo"]);
}

#[test]
fn setup_refuses_a_project_whose_folder_is_the_installer_home() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("plaz", LOGS)]);
    let project = t.path().join("project");
    let home = project.join(".nollm-wiki");
    fs::create_dir(&project).unwrap();
    nollm_wiki_install(&home, &base)
        .arg("install")
        .assert()
        .success();
    let log = t.path().join("log");
    let out = nollm_wiki_install(&home, &base)
        .args(["setup", "--scope", "repo"])
        .current_dir(&project)
        .env("NOLLM_WIKI_TEST_LOG", &log)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!(
        "nollm-wiki-install: {} is nollm-wiki-install's own folder; set the project up from another directory or move \
         NOLLM_WIKI_HOME",
        home.display()
    );
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert!(!log.exists(), "no module runs");
}

#[test]
fn setup_in_home_dir_does_not_collide_with_installer_home() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("plaz", LOGS)]);
    let user = t.path().join("user");
    fs::create_dir_all(user.join(".nollm-wiki")).unwrap();
    let run = |args: &[&str]| {
        // HOME is the project; the installer home is its `.nollm-wiki-home`, the data dir `.nollm-wiki`.
        let mut cmd = nollm_wiki_install(&t.path().join("unused"), &base);
        cmd.env_remove("NOLLM_WIKI_HOME")
            .env("HOME", &user)
            .env("NOLLM_WIKI_TEST_LOG", t.path().join("log"))
            .current_dir(&user)
            .args(args);
        cmd.output().unwrap()
    };
    assert!(run(&["install"]).status.success());
    assert!(user.join(".nollm-wiki-home").is_dir());
    let out = run(&["setup", "--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn setup_with_nothing_installed_says_so() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("wiki", LOGS)]);
    let home = t.path().join("home");
    let out = nollm_wiki_install(&home, &base)
        .args(["setup", "--scope", "repo"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!(
        "nollm-wiki-install: nothing installed in {}; run nollm-wiki-install install",
        home.display()
    );
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert!(!home.exists());
}

#[test]
fn setup_with_a_missing_binary_says_update() {
    let f = installed(LOGS, "wiki,plaz");
    let plaz = f.home.join("bin/plaz");
    fs::remove_file(&plaz).unwrap();
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!(
        "nollm-wiki-install: plaz is installed but {} is missing; run nollm-wiki-install update",
        plaz.display()
    );
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert_eq!(f.logged(), NONE, "no module runs before the check");
}

#[test]
fn setup_memory_without_wiki_is_usage() {
    let f = installed(LOGS, "wiki,plaz");
    for flag in ["--memory", "--no-memory", "--hook", "--no-hook"] {
        let out = f.setup(&["--modules", "plaz", "--scope", "repo", flag]);
        usage(
            &out,
            &format!("nollm-wiki-install: {flag} applies to wiki, which is not chosen"),
        );
    }
    usage(
        &f.setup(&["--scope", "repo", "--memory", "--no-memory"]),
        "cannot be used with",
    );
    assert_eq!(f.logged(), NONE);
}

/// wiki, started from the nollm-wiki home, starts itself from there in the agent configs; the
/// terminal's `PATH` is no matter of setup's.
#[test]
fn setup_says_nothing_about_path_when_bin_is_not_on_it() {
    let f = installed(LOGS, "wiki,plaz");
    let out = f.setup(&["--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    assert_eq!(f.logged().len(), 2);
}

#[test]
fn setup_gives_the_modules_bin_on_their_path() {
    let logs_path: &[u8] = b"#!/bin/sh\necho \"$PATH\" >> \"$NOLLM_WIKI_TEST_LOG\"\n";
    let f = installed(logs_path, "wiki");
    let out = f.setup(&["--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let own = std::env::var_os("PATH").unwrap_or_default();
    let bin = f.home.join("bin");
    assert_eq!(f.logged(), [format!("{}:{}", bin.display(), own.display())]);
}

#[test]
fn setup_with_a_colon_in_the_home_path_runs_the_modules_with_path_unchanged() {
    // `bin/` cannot be put on a PATH (`:` separates its entries): the modules get the PATH as it is.
    let logs_path: &[u8] = b"#!/bin/sh\necho \"$PATH\" >> \"$NOLLM_WIKI_TEST_LOG\"\n";
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("wiki", logs_path)]);
    let home = t.path().join("nollm-wiki-install:home");
    nollm_wiki_install(&home, &base)
        .args(["install", "--modules", "wiki"])
        .assert()
        .success();
    let project = t.path().join("project");
    fs::create_dir(&project).unwrap();
    let log = t.path().join("log");
    let out = nollm_wiki_install(&home, &base)
        .args(["setup", "--scope", "repo"])
        .current_dir(&project)
        .env("NOLLM_WIKI_TEST_LOG", &log)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let own = std::env::var_os("PATH").unwrap_or_default();
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        format!("{}\n", own.display())
    );
}

#[test]
fn setup_without_asking_gives_the_modules_no_input() {
    let reads: &[u8] =
        b"#!/bin/sh\nread -r line; echo \"read [$line]\" >> \"$NOLLM_WIKI_TEST_LOG\"\n";
    let f = installed(reads, "wiki");
    let out = nollm_wiki_install(&f.home, &f.base)
        .args(["setup", "--scope", "repo", "--yes"])
        .current_dir(f.t.path().join("project"))
        .env("NOLLM_WIKI_TEST_LOG", f.log())
        .write_stdin("claude\n")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.logged(), ["read []"]);
}

#[test]
fn setup_agent_without_wiki_is_usage() {
    let f = installed(LOGS, "plaz");
    let out = f.setup(&["--scope", "repo", "--agent", "claude"]);
    usage(
        &out,
        "nollm-wiki-install: --agent applies to wiki, which is not chosen",
    );
    assert_eq!(f.logged(), NONE);
}

/// `out`'s stderr contains the line `nollm-wiki-install: <failed> setup failed (<how>)` for each failure and
/// ends with the summary naming them all.
fn failed(out: &std::process::Output, failures: &[(&str, &str)]) {
    let err = stderr(out);
    for (name, how) in failures {
        let line = format!("nollm-wiki-install: {name} setup failed ({how})");
        assert!(err.lines().any(|l| l == line), "{err}");
    }
    let names: Vec<&str> = failures.iter().map(|f| f.0).collect();
    let last = format!(
        "nollm-wiki-install: setup finished with errors: {}",
        names.join(", ")
    );
    assert_eq!(err.lines().last(), Some(last.as_str()), "{err}");
}

#[test]
fn setup_runs_every_module_even_when_one_fails() {
    let f = installed(LOGS_THEN_FAILS, "wiki,plaz");
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "wiki setup --scope repo --memory",
            "plaz setup --scope repo"
        ]
    );
    failed(&out, &[("wiki", "exit 3")]);
}

#[test]
fn setup_remove_runs_every_module_even_when_one_fails() {
    let f = installed(LOGS_THEN_FAILS, "wiki,plaz");
    // plaz fails too: the status is the first failure's.
    fs::write(f.home.join("bin/plaz"), b"#!/bin/sh\nexit 4\n").unwrap();
    let out = f.setup(&["--remove", "--scope", "repo"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(f.logged(), ["wiki setup --scope repo --memory --remove"]);
    failed(&out, &[("wiki", "exit 3"), ("plaz", "exit 4")]);
}

#[test]
fn setup_runs_no_module_when_one_is_not_executable() {
    use std::os::unix::fs::PermissionsExt;
    let f = installed(LOGS, "wiki,plaz");
    let plaz = f.home.join("bin/plaz");
    fs::set_permissions(&plaz, fs::Permissions::from_mode(0o644)).unwrap();
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!("nollm-wiki-install: {} is not executable", plaz.display());
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert_eq!(f.logged(), NONE);
}

#[test]
fn setup_with_no_module_installed_says_so() {
    let f = installed(LOGS, "plaz");
    nollm_wiki_install(&f.home, &f.base)
        .args(["remove", "--modules", "plaz", "--yes"])
        .assert()
        .success();
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = "nollm-wiki-install: nothing to set up: no module is installed (nollm-wiki-install install)";
    assert!(stderr(&out).contains(expect), "{}", stderr(&out));
}

#[test]
fn setup_of_a_module_killed_by_a_signal_exits_1() {
    let killed: &[u8] = b"#!/bin/sh\nkill -9 $$\n";
    let f = installed(killed, "wiki,plaz");
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    failed(&out, &[("wiki", "signal 9")]);
    assert_eq!(f.logged(), ["plaz setup --scope repo"]);
}
