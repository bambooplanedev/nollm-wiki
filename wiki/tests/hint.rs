//! `wiki hint` end to end: code locations for a task text, silent when there is nothing to say.

use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::{tempdir, TempDir};

const TASK: &str = "where are member creds loaded";

fn project() -> TempDir {
    let tmp = tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(
        tmp.path().join("src/lib.rs"),
        "/// Load a member's locker credentials for each site.\npub fn load_member_creds() {}\n\npub fn parse_order() {}\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("notes.md"),
        "# Member creds\n\nWhere member creds are loaded.\n",
    )
    .unwrap();
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile"])
        .arg(tmp.path())
        .arg(tmp.path().join(".wiki"))
        .assert()
        .success();
    tmp
}

fn hint(dir: &Path, text: &str) -> String {
    let out = Command::cargo_bin("wiki")
        .unwrap()
        .args(["hint", text, "--dir"])
        .arg(dir)
        .assert()
        .success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

#[test]
fn hint_prints_symbol_lines_for_a_task() {
    let tmp = project();
    let out = hint(&tmp.path().join(".wiki"), TASK);
    assert!(
        out.starts_with("Code locations from the project wiki"),
        "{out}"
    );
    assert!(out
        .lines()
        .any(|l| l.starts_with("src/lib.rs:") && l.contains("load_member_creds")));
    assert!(!out.contains("notes.md"), "{out}");
}

#[test]
fn hint_ignores_docs_and_caps_lines() {
    let tmp = project();
    let chatty = "hey so I was wondering if you could maybe tell me where the member creds are \
        actually loaded in this project because it is the thing that I need to know about right now \
        and also what the code does with them when they are there please and thank you very much";
    let out = hint(&tmp.path().join(".wiki"), chatty);
    assert!(out.lines().count() <= 9, "{out}");
    assert!(!out.contains("notes.md"), "{out}");
}

#[test]
fn hint_is_silent_without_a_wiki() {
    let tmp = project();
    assert_eq!(hint(&tmp.path().join("missing"), TASK), "");
}

#[test]
fn hint_is_silent_when_nothing_matches() {
    let tmp = project();
    assert_eq!(hint(&tmp.path().join(".wiki"), "zzqx"), "");
}

#[test]
fn hint_reads_claude_hook_json() {
    let tmp = project();
    let expected = hint(&tmp.path().join(".wiki"), TASK);
    let out = Command::cargo_bin("wiki")
        .unwrap()
        .args(["hint", "--claude-hook"])
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .write_stdin(format!(r#"{{"session_id":"x","prompt":"{TASK}"}}"#))
        .assert()
        .success();
    assert_eq!(
        String::from_utf8(out.get_output().stdout.clone()).unwrap(),
        expected
    );
    assert!(!expected.is_empty());
}
