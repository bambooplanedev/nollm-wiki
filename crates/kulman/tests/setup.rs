mod common;

use std::fs;
use std::path::PathBuf;

use common::{kulman, release, stderr};
use serde_json::{json, Value};

/// A module that appends `<name> <args>` to `$KULMAN_TEST_LOG`.
const LOGS: &[u8] = b"#!/bin/sh\necho \"$(basename \"$0\") $*\" >> \"$KULMAN_TEST_LOG\"\n";
/// The same, then exits 3.
const LOGS_THEN_FAILS: &[u8] =
    b"#!/bin/sh\necho \"$(basename \"$0\") $*\" >> \"$KULMAN_TEST_LOG\"\nexit 3\n";

/// An empty log: no module ran.
const NONE: [&str; 0] = [];

/// A temp root with `release/` (kalka and plaz), `home` holding `modules`, and `project/`.
struct Fixture {
    t: tempfile::TempDir,
    base: String,
    home: PathBuf,
}

fn installed(kalka: &[u8], modules: &str) -> Fixture {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("kalka", kalka), ("plaz", LOGS)],
    );
    let home = t.path().join("home");
    kulman(&home, &base)
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

    /// `kulman setup <args>` in the project dir, stdin closed.
    fn setup(&self, args: &[&str]) -> std::process::Output {
        kulman(&self.home, &self.base)
            .arg("setup")
            .args(args)
            .current_dir(self.t.path().join("project"))
            .env("KULMAN_TEST_LOG", self.log())
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
    let f = installed(LOGS, "kalka,plaz");
    let out = f.setup(&["--agent", "claude", "--scope", "repo", "--no-memory"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "kalka setup --agent claude --scope repo --no-memory",
            "plaz setup --scope repo"
        ]
    );
    let out = f.setup(&[
        "--modules",
        "kalka",
        "--scope",
        "local",
        "--no-hook",
        "--yes",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged()[2..],
        ["kalka setup --scope local --no-hook --memory"]
    );
}

#[test]
fn setup_defaults_to_every_installed_module_with_memory() {
    let f = installed(LOGS, "kalka,plaz");
    let out = f.setup(&["--scope", "local"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "kalka setup --scope local --memory",
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
    let f = installed(LOGS, "kalka,plaz");
    let path = f.home.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["modules"][0]["options"] =
        json!([{"name": "memory", "summary": "agent memory", "default": false}]);
    fs::write(&path, manifest.to_string()).unwrap();
    let out = f.setup(&["--modules", "kalka", "--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = f.setup(&["--modules", "kalka", "--scope", "repo", "--memory"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "kalka setup --scope repo --no-memory",
            "kalka setup --scope repo --memory"
        ]
    );
}

#[test]
fn setup_without_scope_is_usage() {
    let f = installed(LOGS, "kalka,plaz");
    for args in [&[][..], &["--yes"], &["--modules", "plaz"]] {
        usage(
            &f.setup(args),
            "kulman: --scope is required (repo or local)",
        );
    }
    assert_eq!(f.logged(), NONE);
}

#[test]
fn setup_remove_without_scope_is_usage_when_kalka_is_chosen() {
    let f = installed(LOGS, "kalka,plaz");
    for args in [&["--remove"][..], &["--remove", "--modules", "kalka"]] {
        usage(
            &f.setup(args),
            "kulman: --scope is required (repo or local)",
        );
    }
    assert_eq!(f.logged(), NONE);
}

#[test]
fn setup_remove_of_plaz_alone_needs_no_scope() {
    let f = installed(LOGS, "kalka,plaz");
    let out = f.setup(&["--remove", "--modules", "plaz"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.logged(), ["plaz setup --remove"]);
}

#[test]
fn setup_remove_reaches_every_module() {
    let f = installed(LOGS, "kalka,plaz");
    let out = f.setup(&["--remove", "--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "kalka setup --scope repo --memory --remove",
            "plaz setup --scope repo --remove"
        ]
    );
}

#[test]
fn setup_of_a_module_not_installed_says_how() {
    let f = installed(LOGS, "plaz");
    let out = f.setup(&["--modules", "kalka", "--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = "kulman: kalka is not installed; run kulman install --modules kalka";
    assert!(stderr(&out).contains(expect), "{}", stderr(&out));
    usage(
        &f.setup(&["--modules", "kulman", "--scope", "repo"]),
        "kulman: kulman is not a module",
    );
    // Not in the release either: no install hint, which would fail.
    let out = f.setup(&["--modules", "nope", "--scope", "repo"]);
    usage(
        &out,
        "kulman: --modules: unknown module \"nope\" (known: kalka, plaz)",
    );
    assert!(!stderr(&out).contains("install"), "{}", stderr(&out));
    usage(
        &f.setup(&["--modules", "plaz,", "--scope", "repo"]),
        "kulman: --modules: bad name \"\"",
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
    let home = project.join(".kulman");
    fs::create_dir(&project).unwrap();
    kulman(&home, &base).arg("install").assert().success();
    let log = t.path().join("log");
    let out = kulman(&home, &base)
        .args(["setup", "--scope", "repo"])
        .current_dir(&project)
        .env("KULMAN_TEST_LOG", &log)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!(
        "kulman: {} is kulman's own folder; set the project up from another directory or move \
         KULMAN_HOME",
        home.display()
    );
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert!(!log.exists(), "no module runs");
}

#[test]
fn setup_with_nothing_installed_says_so() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("kalka", LOGS)]);
    let home = t.path().join("home");
    let out = kulman(&home, &base)
        .args(["setup", "--scope", "repo"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("kulman: nothing installed; run kulman install"),
        "{}",
        stderr(&out)
    );
    assert!(!home.exists());
}

#[test]
fn setup_with_a_missing_binary_says_update() {
    let f = installed(LOGS, "kalka,plaz");
    let plaz = f.home.join("bin/plaz");
    fs::remove_file(&plaz).unwrap();
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!(
        "kulman: plaz is installed but {} is missing; run kulman update",
        plaz.display()
    );
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert_eq!(f.logged(), NONE, "no module runs before the check");
}

#[test]
fn setup_memory_without_kalka_is_usage() {
    let f = installed(LOGS, "kalka,plaz");
    for flag in ["--memory", "--no-memory", "--no-hook"] {
        let out = f.setup(&["--modules", "plaz", "--scope", "repo", flag]);
        usage(
            &out,
            &format!("kulman: {flag} applies to kalka, which is not chosen"),
        );
    }
    usage(
        &f.setup(&["--scope", "repo", "--memory", "--no-memory"]),
        "cannot be used with",
    );
    assert_eq!(f.logged(), NONE);
}

/// `PATH` with `dir` first, then the test's own.
fn path_with(dir: &std::path::Path) -> std::ffi::OsString {
    let own = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&own));
    std::env::join_paths(dirs).unwrap()
}

#[test]
fn setup_warns_when_bin_is_not_on_path_and_still_runs() {
    let f = installed(LOGS, "kalka,plaz");
    let bin = f.home.join("bin");
    let out = f.setup(&["--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let warning = [
        format!("kulman: {} is not on PATH", bin.display()),
        "kulman: the agent configs written now start \"kalka\"; an agent started without it on \
         PATH will not find it"
            .to_string(),
        format!("kulman: add to PATH: {}", bin.display()),
    ];
    assert_eq!(stderr(&out).lines().collect::<Vec<_>>(), warning);
    assert_eq!(f.logged().len(), 2);
    // On PATH: nothing said.
    let out = kulman(&f.home, &f.base)
        .args(["setup", "--scope", "repo"])
        .current_dir(f.t.path().join("project"))
        .env("KULMAN_TEST_LOG", f.log())
        .env("PATH", path_with(&bin))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    assert_eq!(f.logged().len(), 4);
}

#[test]
fn setup_gives_the_modules_bin_on_their_path() {
    let logs_path: &[u8] = b"#!/bin/sh\necho \"$PATH\" >> \"$KULMAN_TEST_LOG\"\n";
    let f = installed(logs_path, "kalka");
    let out = f.setup(&["--scope", "repo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let own = std::env::var_os("PATH").unwrap_or_default();
    let bin = f.home.join("bin");
    assert_eq!(f.logged(), [format!("{}:{}", bin.display(), own.display())]);
}

#[test]
fn setup_without_asking_gives_the_modules_no_input() {
    let reads: &[u8] = b"#!/bin/sh\nread -r line; echo \"read [$line]\" >> \"$KULMAN_TEST_LOG\"\n";
    let f = installed(reads, "kalka");
    let out = kulman(&f.home, &f.base)
        .args(["setup", "--scope", "repo", "--yes"])
        .current_dir(f.t.path().join("project"))
        .env("KULMAN_TEST_LOG", f.log())
        .write_stdin("claude\n")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.logged(), ["read []"]);
}

#[test]
fn setup_agent_without_kalka_is_usage() {
    let f = installed(LOGS, "plaz");
    let out = f.setup(&["--scope", "repo", "--agent", "claude"]);
    usage(
        &out,
        "kulman: --agent applies to kalka, which is not chosen",
    );
    assert_eq!(f.logged(), NONE);
}

/// `out`'s stderr contains the line `kulman: <failed> setup failed (<how>)` for each failure and
/// ends with the summary naming them all.
fn failed(out: &std::process::Output, failures: &[(&str, &str)]) {
    let err = stderr(out);
    for (name, how) in failures {
        let line = format!("kulman: {name} setup failed ({how})");
        assert!(err.lines().any(|l| l == line), "{err}");
    }
    let names: Vec<&str> = failures.iter().map(|f| f.0).collect();
    let last = format!("kulman: setup finished with errors: {}", names.join(", "));
    assert_eq!(err.lines().last(), Some(last.as_str()), "{err}");
}

#[test]
fn setup_runs_every_module_even_when_one_fails() {
    let f = installed(LOGS_THEN_FAILS, "kalka,plaz");
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(
        f.logged(),
        [
            "kalka setup --scope repo --memory",
            "plaz setup --scope repo"
        ]
    );
    failed(&out, &[("kalka", "exit 3")]);
}

#[test]
fn setup_remove_runs_every_module_even_when_one_fails() {
    let f = installed(LOGS_THEN_FAILS, "kalka,plaz");
    // plaz fails too: the status is the first failure's.
    fs::write(f.home.join("bin/plaz"), b"#!/bin/sh\nexit 4\n").unwrap();
    let out = f.setup(&["--remove", "--scope", "repo"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(f.logged(), ["kalka setup --scope repo --memory --remove"]);
    failed(&out, &[("kalka", "exit 3"), ("plaz", "exit 4")]);
}

#[test]
fn setup_of_a_module_exiting_2_exits_2_and_says_it_was_the_module() {
    let f = installed(b"#!/bin/sh\nexit 2\n", "kalka,plaz");
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    failed(&out, &[("kalka", "exit 2")]);
    assert_eq!(f.logged(), ["plaz setup --scope repo"]);
}

#[test]
fn setup_runs_no_module_when_one_is_not_executable() {
    use std::os::unix::fs::PermissionsExt;
    let f = installed(LOGS, "kalka,plaz");
    let plaz = f.home.join("bin/plaz");
    fs::set_permissions(&plaz, fs::Permissions::from_mode(0o644)).unwrap();
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!("kulman: {} is not executable", plaz.display());
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert_eq!(f.logged(), NONE);
}

#[test]
fn setup_with_no_module_installed_says_so() {
    let f = installed(LOGS, "plaz");
    kulman(&f.home, &f.base)
        .args(["remove", "--modules", "plaz", "--yes"])
        .assert()
        .success();
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = "kulman: nothing to set up: no module is installed (kulman install)";
    assert!(stderr(&out).contains(expect), "{}", stderr(&out));
}

#[test]
fn setup_of_a_module_killed_by_a_signal_exits_1() {
    let killed: &[u8] = b"#!/bin/sh\nkill -9 $$\n";
    let f = installed(killed, "kalka,plaz");
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    failed(&out, &[("kalka", "signal 9")]);
    assert_eq!(f.logged(), ["plaz setup --scope repo"]);
}
