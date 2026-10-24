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
    assert!(out.contains("load_member_creds"), "{out}");
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

#[test]
fn hint_text_may_start_with_a_dash() {
    let tmp = project();
    let dir = tmp.path().join(".wiki");
    let out = Command::cargo_bin("wiki")
        .unwrap()
        .args(["hint", "--dir"])
        .arg(&dir)
        .args(["--", "-- where are member creds loaded"])
        .assert()
        .success();
    let out = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(out.contains("load_member_creds"), "{out}");
}

#[test]
fn hint_claude_hook_is_silent_on_non_utf8_stdin() {
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["hint", "--claude-hook"])
        .write_stdin(vec![0xff, 0xfe, 0xfd])
        .assert()
        .success()
        .stdout("");
}

/// `project()` plus a memory folder with one note (title, alias, summary paragraph, body).
fn project_with_memory() -> TempDir {
    let tmp = tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::create_dir_all(tmp.path().join(".agents/memory")).unwrap();
    fs::write(
        tmp.path().join("src/lib.rs"),
        "/// Load a member's locker credentials for each site.\npub fn load_member_creds() {}\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join(".agents/memory/vault.md"),
        "---\ntitle: Vault rotation\naliases: [keyring]\n---\n\nCredentials rotate nightly.\n\nBody mentions member and zebra and loaded things.\n",
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

fn memory_line(out: &str) -> Option<&str> {
    out.lines().find(|l| l.starts_with("memory: "))
}

#[test]
fn hint_adds_one_memory_line_after_the_code_lines() {
    let tmp = project_with_memory();
    let dir = tmp.path().join(".wiki");
    // Title word, alias word and summary word (all 6+ characters) each gate the line in.
    for prompt in ["rotation", "keyring", "nightly"] {
        let out = hint(&dir, &format!("{prompt} where are member creds loaded"));
        assert!(out.starts_with("Code locations"), "{out}");
        assert_eq!(out.matches("memory: ").count(), 1, "{out}");
        assert!(out.lines().last().unwrap().starts_with("memory: "), "{out}");
        assert!(
            memory_line(&out)
                .unwrap()
                .contains("Credentials rotate nightly."),
            "{out}"
        );
        assert!(memory_line(&out)
            .unwrap()
            .contains(".agents/memory/vault.md"));
    }
}

#[test]
fn hint_memory_line_needs_a_title_alias_or_summary_match() {
    let tmp = project_with_memory();
    // Only the body says zebra: no memory line, and no code either.
    assert_eq!(hint(&tmp.path().join(".wiki"), "zebra"), "");
}

#[test]
fn hint_memory_line_alone_gets_the_header() {
    let tmp = project_with_memory();
    let out = hint(&tmp.path().join(".wiki"), "vault rotation");
    assert!(out.starts_with("Code locations"), "{out}");
    assert_eq!(out.lines().count(), 2, "{out}");
}

#[test]
fn hint_code_lines_are_the_same_with_and_without_memory() {
    let with = project_with_memory();
    let without = project();
    let code = |s: String| {
        s.lines()
            .filter(|l| !l.starts_with("memory: "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let prompt = "vault where are member creds loaded";
    assert_eq!(
        code(hint(&with.path().join(".wiki"), prompt)),
        code(hint(&without.path().join(".wiki"), prompt))
    );
}

#[test]
fn hint_memory_gate_ignores_short_words() {
    let tmp = tempdir().unwrap();
    fs::create_dir_all(tmp.path().join(".agents/memory")).unwrap();
    fs::write(
        tmp.path().join(".agents/memory/n.md"),
        "---\ntitle: Що робити в релізі\n---\n\nРелізний чекліст.\n",
    )
    .unwrap();
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile"])
        .arg(tmp.path())
        .arg(tmp.path().join(".wiki"))
        .assert()
        .success();
    let dir = tmp.path().join(".wiki");
    assert_eq!(hint(&dir, "що в"), "");
    assert!(hint(&dir, "що в релізі").contains("memory: "));
}

#[test]
fn hint_never_shows_a_superseded_note() {
    let tmp = project_with_memory();
    fs::write(
        tmp.path().join(".agents/memory/vault2.md"),
        "---\ntitle: Vault weekly\nsupersedes: Vault rotation\ndescription: Credentials rotate weekly.\n---\n\nbody\n",
    )
    .unwrap();
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile"])
        .arg(tmp.path())
        .arg(tmp.path().join(".wiki"))
        .assert()
        .success();
    let out = hint(&tmp.path().join(".wiki"), "vault rotation nightly");
    let line = memory_line(&out).unwrap();
    assert!(
        line.contains("vault2.md") && !line.contains("vault.md"),
        "{out}"
    );
}

/// A code page that says `cache` in every doc line, plus a note naming it once
/// (`cache` is 5 characters, `keyring` 7).
fn project_with_loud_code() -> TempDir {
    let tmp = tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::create_dir_all(tmp.path().join(".agents/memory")).unwrap();
    fs::write(
        tmp.path().join("src/lib.rs"),
        "/// Cache cache cache: the cache of cached cache entries.\npub fn cache_cache() {}\n\n/// Cache keyring keyring lookups.\npub fn cache_keyring() {}\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join(".agents/memory/n.md"),
        "---\ntitle: Eviction notes\naliases: [keyring]\n---\n\nEntries live in the cache.\n",
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

#[test]
fn hint_memory_line_skips_a_short_word_the_code_outscores() {
    let tmp = project_with_loud_code();
    let out = hint(&tmp.path().join(".wiki"), "cache");
    assert!(out.contains("cache_cache"), "{out}");
    assert!(memory_line(&out).is_none(), "{out}");
}

#[test]
fn hint_memory_line_keeps_a_short_word_when_no_code_competes() {
    let tmp = project_with_memory();
    // `vault` (5 characters) hits the title; no code page mentions it.
    let out = hint(&tmp.path().join(".wiki"), "vault");
    assert!(memory_line(&out).is_some(), "{out}");
}

#[test]
fn hint_memory_line_keeps_a_long_word_the_code_outscores() {
    let tmp = project_with_loud_code();
    let out = hint(&tmp.path().join(".wiki"), "cache keyring");
    assert!(out.contains("cache_keyring"), "{out}");
    assert!(memory_line(&out).is_some(), "{out}");
}
