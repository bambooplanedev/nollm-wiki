//! `wiki hint` end to end: code locations for a task text, silent when there is nothing to say.

use assert_cmd::Command;
use std::path::Path;
use tempfile::{tempdir, TempDir};

mod common;
use common::write;

const TASK: &str = "where are member creds loaded";

/// `wiki compile <root> <root>/.wiki` through the CLI.
fn compile(root: &Path) {
    Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile"])
        .arg(root)
        .arg(root.join(".wiki"))
        .assert()
        .success();
}

/// A fresh project holding `files`, compiled into its `.wiki`.
fn project_of(files: &[(&str, &str)]) -> TempDir {
    let tmp = tempdir().unwrap();
    for (name, body) in files {
        write(tmp.path(), name, body);
    }
    compile(tmp.path());
    tmp
}

fn project() -> TempDir {
    project_of(&[
        (
            "src/lib.rs",
            "/// Load a member's locker credentials for each site.\npub fn load_member_creds() {}\n\npub fn parse_order() {}\n",
        ),
        ("notes.md", "# Member creds\n\nWhere member creds are loaded.\n"),
    ])
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

fn hint_in(tmp: &TempDir, text: &str) -> String {
    hint(&tmp.path().join(".wiki"), text)
}

#[test]
fn hint_prints_symbol_lines_for_a_task() {
    let tmp = project();
    let out = hint_in(&tmp, TASK);
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
    let out = hint_in(&tmp, chatty);
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
    assert_eq!(hint_in(&tmp, "zzqx"), "");
}

#[test]
fn hint_reads_claude_hook_json() {
    let tmp = project();
    let expected = hint_in(&tmp, TASK);
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
    assert_ne!(expected, "");
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
    project_of(&[
        (
            "src/lib.rs",
            "/// Load a member's locker credentials for each site.\npub fn load_member_creds() {}\n",
        ),
        (
            ".agents/memory/vault.md",
            "---\ntitle: Vault rotation\naliases: [keyring]\n---\n\nCredentials rotate nightly.\n\nBody mentions member and zebra and loaded things.\n",
        ),
    ])
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
fn hint_memory_line_comes_from_the_body_when_no_code_competes() {
    let tmp = project_with_memory();
    // Only the body says zebra and no code does: the ratio branch lets it in.
    let out = hint_in(&tmp, "zebra");
    assert!(memory_line(&out).is_some(), "{out}");
    assert_eq!(out.lines().count(), 2, "{out}");
}

#[test]
fn hint_gate_length_comes_from_the_env() {
    let tmp = project_with_memory();
    let prompt = "rota where are member creds loaded";
    // `rota` (4 characters) names the note's title only under a gate of 4.
    let gated = Command::cargo_bin("wiki")
        .unwrap()
        .args(["hint", prompt, "--dir"])
        .arg(tmp.path().join(".wiki"))
        .env("WIKI_MEMORY_GATE", "4")
        .assert()
        .success();
    let gated = String::from_utf8(gated.get_output().stdout.clone()).unwrap();
    assert!(gated.contains("load_member_creds"), "{gated}");
    assert!(memory_line(&gated).is_some(), "{gated}");
    let plain = hint_in(&tmp, prompt);
    assert!(memory_line(&plain).is_none(), "{plain}");
}

#[test]
fn hint_memory_line_alone_gets_the_header() {
    let tmp = project_with_memory();
    let out = hint_in(&tmp, "vault rotation");
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
        code(hint_in(&with, prompt)),
        code(hint_in(&without, prompt))
    );
}

#[test]
fn hint_memory_gate_ignores_short_words() {
    // Code that says the short words loudly, so the ratio branch fails too.
    let tmp = project_of(&[
        ("src/lib.rs", "/// Що в що в що в що.\npub fn shcho() {}\n"),
        (
            ".agents/memory/n.md",
            "---\ntitle: Що робити в релізі\n---\n\nРелізний чекліст.\n",
        ),
    ]);
    let dir = tmp.path().join(".wiki");
    assert!(!hint(&dir, "що в").contains("memory: "));
    assert!(hint(&dir, "що в чекліст").contains("memory: "));
}

#[test]
fn hint_never_shows_a_superseded_note() {
    let tmp = project_with_memory();
    write(
        tmp.path(),
        ".agents/memory/vault2.md",
        "---\ntitle: Vault weekly\nsupersedes: Vault rotation\ndescription: Credentials rotate weekly.\n---\n\nbody\n",
    );
    compile(tmp.path());
    let out = hint_in(&tmp, "vault rotation nightly");
    let line = memory_line(&out).unwrap();
    assert!(
        line.contains("vault2.md") && !line.contains("vault.md"),
        "{out}"
    );
}

/// A code page that says `cache` in every doc line, plus a note naming it once
/// (`cache` is 5 characters, `keyring` 7).
fn project_with_loud_code() -> TempDir {
    project_of(&[
        (
            "src/lib.rs",
            "/// Cache cache cache: the cache of cached cache entries.\npub fn cache_cache() {}\n\n/// Cache keyring keyring lookups.\npub fn cache_keyring() {}\n",
        ),
        (
            ".agents/memory/n.md",
            "---\ntitle: Eviction notes\naliases: [keyring]\n---\n\nEntries live in the cache.\n",
        ),
    ])
}

#[test]
fn hint_memory_line_skips_a_short_word_the_code_outscores() {
    let tmp = project_with_loud_code();
    let out = hint_in(&tmp, "cache");
    assert!(out.contains("cache_cache"), "{out}");
    assert!(memory_line(&out).is_none(), "{out}");
}

#[test]
fn hint_memory_line_keeps_a_short_word_when_no_code_competes() {
    let tmp = project_with_memory();
    // `vault` (5 characters) is under the gate length, but no code page
    // mentions it, so the note's own score passes the ratio branch.
    let out = hint_in(&tmp, "vault");
    assert!(memory_line(&out).is_some(), "{out}");
}

#[test]
fn hint_memory_line_keeps_a_long_word_the_code_outscores() {
    let tmp = project_with_loud_code();
    let out = hint_in(&tmp, "cache keyring");
    assert!(out.contains("cache_keyring"), "{out}");
    assert!(memory_line(&out).is_some(), "{out}");
}

#[test]
fn hint_shows_the_journal_of_a_date_in_the_prompt() {
    let tmp = tempdir().unwrap();
    write(tmp.path(), "src/lib.rs", "pub fn stand() {}\n");
    for day in ["05", "12"] {
        write(
            tmp.path(),
            &format!(".agents/memory/journal/2026-10-{day}.md"),
            &format!("---\ntitle: Journal {day}\n---\n\nstand run\n"),
        );
    }
    compile(tmp.path());
    let out = hint_in(&tmp, "Що сталося зі стендом 12 жовтня?");
    assert!(memory_line(&out).unwrap().contains("2026-10-12"), "{out}");
    assert!(memory_line(&hint_in(&tmp, "Що сталося зі стендом?")).is_none());
}

#[test]
fn hint_skips_vendored_code() {
    let tmp = project_of(&[
        (
            "third_party/cli/cli.hpp",
            "void parse_flags_a() {}\nvoid parse_flags_b() {}\n",
        ),
        ("vendor/lib/lib.rs", "pub fn parse_flags_c() {}\n"),
        ("src/args.rs", "pub fn parse_flags_d() {}\n"),
    ]);
    let out = hint_in(&tmp, "parse flags");
    assert!(out.contains("src/args.rs"), "{out}");
    assert!(!out.contains("third_party/"), "{out}");
    assert!(!out.contains("vendor/"), "{out}");
}

#[test]
fn hint_of_a_long_prompt_counts_only_its_rare_words() {
    // Three pages hold 30 common prompt words; 24 rarer words have a page each.
    let common: Vec<String> = (0..30).map(|i| format!("common{i:02}")).collect();
    let rare: Vec<String> = (0..24).map(|i| format!("rare{i:02}")).collect();
    let body = common
        .iter()
        .map(|w| format!("fn {w}() {{}}"))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    let mut files: Vec<(String, String)> = (0..3)
        .map(|i| (format!("src/process{i}.rs"), body.clone()))
        .collect();
    files.extend(
        rare.iter()
            .map(|w| (format!("src/{w}.rs"), format!("fn {w}() {{}}\n"))),
    );
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    let tmp = project_of(&refs);
    let out = hint_in(&tmp, &format!("{} {}", common.join(" "), rare.join(" ")));
    assert!(out.contains("src/rare"), "{out}");
    assert!(!out.contains("src/process"), "{out}");
}

#[test]
fn hint_names_a_page_with_no_matching_symbol() {
    let tmp = project_of(&[(
        "scripts/run.sh",
        "#!/bin/sh\n# Launch the engine.\nexec engine --sky-secret \"$KEY\"\n",
    )]);
    let out = hint_in(&tmp, "sky-secret");
    assert!(
        out.lines().any(|l| l.starts_with("scripts/run.sh ")),
        "{out}"
    );
}
