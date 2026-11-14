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

#[test]
fn setup_stops_at_the_first_failing_module() {
    let f = installed(LOGS_THEN_FAILS, "kalka,plaz");
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(f.logged(), ["kalka setup --scope repo --memory"]);
    assert_eq!(
        stderr(&out).lines().last(),
        Some("kulman: kalka setup failed (exit 3)")
    );
}

#[test]
fn setup_of_a_module_exiting_2_exits_2_and_says_it_was_the_module() {
    let f = installed(b"#!/bin/sh\nexit 2\n", "kalka,plaz");
    let out = f.setup(&["--scope", "repo"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(
        stderr(&out).lines().last(),
        Some("kulman: kalka setup failed (exit 2)")
    );
    assert_eq!(f.logged(), NONE);
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
    assert_eq!(
        stderr(&out).lines().last(),
        Some("kulman: kalka setup failed (signal 9)")
    );
    assert_eq!(f.logged(), NONE);
}
