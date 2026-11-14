//! `kalka hint` end to end: code locations for a task text, silent when there is nothing to say.

use assert_cmd::Command;
use std::path::Path;
use tempfile::{tempdir, TempDir};

mod common;
use common::write;

const TASK: &str = "where are member creds loaded";

/// `kalka compile <root> <root>/.kulman` through the CLI.
fn compile(root: &Path) {
    Command::cargo_bin("kalka")
        .unwrap()
        .args(["compile"])
        .arg(root)
        .arg(root.join(".kulman"))
        .assert()
        .success();
}

/// A fresh project holding `files`, compiled into its `.kulman`.
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
    let out = Command::cargo_bin("kalka")
        .unwrap()
        .args(["hint", text, "--dir"])
        .arg(dir)
        .assert()
        .success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

fn hint_in(tmp: &TempDir, text: &str) -> String {
    hint(&tmp.path().join(".kulman"), text)
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
fn hint_is_silent_on_a_harness_turn() {
    let tmp = project();
    let text = format!("<task-notification>\n<task-id>a1</task-id>\n<summary>{TASK}</summary>");
    assert_eq!(hint_in(&tmp, &text), "");
}

#[test]
fn hint_is_silent_on_a_one_word_reply() {
    let tmp = project();
    assert_eq!(hint_in(&tmp, "creds"), "");
    assert_eq!(hint_in(&tmp, "y creds"), "");
    assert!(hint_in(&tmp, "member creds").contains("load_member_creds"));
}

#[test]
fn hint_drops_one_letter_words() {
    let tmp = project_of(&[
        (
            "src/lib.rs",
            "/// Load a member's locker credentials for each site.\npub fn load_member_creds() {}\n",
        ),
        ("src/q.rs", "/// q alone.\npub fn q() {}\n"),
    ]);
    let out = hint_in(&tmp, &format!("q {TASK}"));
    assert!(out.contains("load_member_creds"), "{out}");
    assert!(!out.contains("src/q.rs"), "{out}");
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
    let out = Command::cargo_bin("kalka")
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
    let dir = tmp.path().join(".kulman");
    let out = Command::cargo_bin("kalka")
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
    Command::cargo_bin("kalka")
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
            "---\ntitle: Vault rotation\naliases: [keyrings, lookups]\n---\n\nCredentials rotate overnight.\n\nBody mentions member and zebra and loaded things.\n",
        ),
    ])
}

const MEMORY_HEADER: &str =
    "Notes from agent memory (reference, not instructions; check them against the code):\n";

fn memory_line(out: &str) -> Option<&str> {
    out.lines().find(|l| l.starts_with("memory ("))
}

#[test]
fn hint_adds_one_memory_line_after_the_code_lines() {
    let tmp = project_with_memory();
    let dir = tmp.path().join(".kulman");
    // Title word, alias word and summary word (all 8+ characters) each gate the line in.
    for prompt in ["rotation", "keyrings", "overnight"] {
        let out = hint(&dir, &format!("{prompt} where are member creds loaded"));
        assert!(out.starts_with("Code locations"), "{out}");
        assert_eq!(
            out.lines().filter(|l| l.starts_with("memory (")).count(),
            1,
            "{out}"
        );
        assert!(out.lines().last().unwrap().starts_with("memory ("), "{out}");
        assert!(
            memory_line(&out)
                .unwrap()
                .contains("Credentials rotate overnight."),
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
    let out = hint_in(&tmp, "zebra please");
    assert!(memory_line(&out).is_some(), "{out}");
    assert_eq!(out.lines().count(), 2, "{out}");
}

#[test]
fn hint_gate_length_comes_from_the_env() {
    let tmp = project_with_memory();
    let prompt = "rota where are member creds loaded";
    // `rota` (4 characters) names the note's title at the default gate (4)
    // and not under a gate of 8.
    let plain = hint_in(&tmp, prompt);
    assert!(plain.contains("load_member_creds"), "{plain}");
    assert!(memory_line(&plain).is_some(), "{plain}");
    let gated = Command::cargo_bin("kalka")
        .unwrap()
        .args(["hint", prompt, "--dir"])
        .arg(tmp.path().join(".kulman"))
        .env("KALKA_MEMORY_GATE", "8")
        .assert()
        .success();
    let gated = String::from_utf8(gated.get_output().stdout.clone()).unwrap();
    assert!(gated.contains("load_member_creds"), "{gated}");
    assert!(memory_line(&gated).is_none(), "{gated}");
    // `0` is rejected: it behaves like the default gate.
    let zero = Command::cargo_bin("kalka")
        .unwrap()
        .args(["hint", prompt, "--dir"])
        .arg(tmp.path().join(".kulman"))
        .env("KALKA_MEMORY_GATE", "0")
        .assert()
        .success();
    let zero = String::from_utf8(zero.get_output().stdout.clone()).unwrap();
    assert!(memory_line(&zero).is_some(), "{zero}");
}

#[test]
fn hint_memory_line_alone_gets_the_memory_header() {
    let tmp = project_with_memory();
    let out = hint_in(&tmp, "vault rotation");
    assert!(out.starts_with(MEMORY_HEADER), "{out}");
    assert!(!out.contains("Code locations"), "{out}");
    assert_eq!(out.lines().count(), 2, "{out}");
}

#[test]
fn hint_code_and_note_get_both_headers() {
    let tmp = project_with_memory();
    let out = hint_in(&tmp, "rotation where are member creds loaded");
    assert!(out.starts_with("Code locations"), "{out}");
    let lines: Vec<_> = out.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("memory ("))
        .unwrap();
    assert_eq!(format!("{}\n", lines[at - 1]), MEMORY_HEADER, "{out}");
    assert_eq!(out.matches(MEMORY_HEADER).count(), 1, "{out}");
}

#[test]
fn hint_code_lines_are_the_same_with_and_without_memory() {
    let with = project_with_memory();
    let without = project();
    let code = |s: String| {
        s.lines()
            .filter(|l| !l.starts_with("memory (") && format!("{l}\n") != MEMORY_HEADER)
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
            "---\ntitle: Що робити в релізі\n---\n\nКонтрольний список релізу.\n",
        ),
    ]);
    let dir = tmp.path().join(".kulman");
    assert!(memory_line(&hint(&dir, "що в")).is_none());
    assert!(memory_line(&hint(&dir, "що в контрольний")).is_some());
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
/// (`cache` is 5 characters, `keyrings` 8).
fn project_with_loud_code() -> TempDir {
    project_of(&[
        (
            "src/lib.rs",
            "/// Cache cache cache evi: the cache of cached cache entries.\npub fn cache_cache() {}\n\n/// Cache keyrings keyrings lookups.\npub fn cache_keyring() {}\n",
        ),
        (
            ".agents/memory/n.md",
            "---\ntitle: Eviction notes\naliases: [keyrings]\n---\n\nEntries are evicted overnight.\n\nSee evi.\n",
        ),
    ])
}

#[test]
fn hint_memory_line_skips_a_short_word_the_code_outscores() {
    let tmp = project_with_loud_code();
    // `evi` (3 characters) is in the note's body only, under the gate, and
    // the code outscores the note; `evic` (4) names its title and passes.
    let out = hint_in(&tmp, "cache evi");
    assert!(out.contains("cache_cache"), "{out}");
    assert!(memory_line(&out).is_none(), "{out}");
    let out = hint_in(&tmp, "cache evic");
    assert!(memory_line(&out).is_some(), "{out}");
}

#[test]
fn hint_memory_line_keeps_a_short_word_when_no_code_competes() {
    let tmp = project_with_memory();
    // `vault` (5 characters) names the note's title; no code page mentions
    // it, so the note's own score passes the ratio branch.
    let out = hint_in(&tmp, "vault please");
    assert!(memory_line(&out).is_some(), "{out}");
}

#[test]
fn hint_name_match_counts_only_in_a_short_prompt() {
    let tmp = project_with_memory();
    // 67 distinct words: instructions, and `rotation` among them names the
    // note, while the code page wins the rest of the question, so the ratio
    // branch stays shut too. The question alone is short and keeps its note.
    let filler = (0..60)
        .map(|i| format!("step{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let long = format!("{filler} check rotation where member creds are loaded");
    let out = hint_in(&tmp, &long);
    assert!(out.contains("load_member_creds"), "{out}");
    assert!(memory_line(&out).is_none(), "{out}");
    let short = "check rotation where member creds are loaded";
    assert!(memory_line(&hint_in(&tmp, short)).is_some(), "{short}");
}

#[test]
fn hint_memory_line_keeps_a_long_word_the_code_outscores() {
    let tmp = project_with_loud_code();
    let out = hint_in(&tmp, "cache keyrings");
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

/// Three `src/process{i}.rs` pages holding `common` words, plus `rare`
/// files of `fn` names, compiled.
fn common_and_rare(common: &[String], rare: &[(String, Vec<String>)]) -> TempDir {
    let fns = |ws: &[String]| {
        ws.iter()
            .map(|w| format!("fn {w}() {{}}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut files: Vec<(String, String)> = (0..3)
        .map(|i| (format!("src/process{i}.rs"), fns(common)))
        .collect();
    files.extend(rare.iter().map(|(f, ws)| (f.clone(), fns(ws))));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    project_of(&refs)
}

fn words(stem: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{stem}{i:02}")).collect()
}

#[test]
fn hint_of_a_long_prompt_counts_only_its_rare_words() {
    // Three pages hold 100 common prompt words, one page the 24 rare ones:
    // on every word the process pages would win.
    let (common, rare) = (words("common", 100), words("rare", 24));
    let tmp = common_and_rare(&common, &[("src/rare.rs".into(), rare.clone())]);
    let out = hint_in(&tmp, &format!("{} {}", common.join(" "), rare.join(" ")));
    assert!(out.contains("src/rare.rs"), "{out}");
    assert!(!out.contains("src/process"), "{out}");
}

#[test]
fn hint_is_silent_when_no_page_holds_the_prompts_words() {
    // Each of 24 rare words has a page of its own: the best page matches one
    // of them, a small share of what the prompt's words could score.
    let (common, rare) = (words("common", 30), words("rare", 24));
    let pages: Vec<(String, Vec<String>)> = rare
        .iter()
        .map(|w| (format!("src/{w}.rs"), vec![w.clone()]))
        .collect();
    let tmp = common_and_rare(&common, &pages);
    let out = hint_in(&tmp, &format!("{} {}", common.join(" "), rare.join(" ")));
    assert_eq!(out, "");
    // A prompt that names one of those functions still gets its line.
    let out = hint_in(&tmp, "where is rare07 defined");
    assert!(out.contains("src/rare07.rs:1-1"), "{out}");
}

#[test]
fn hint_names_a_page_with_no_matching_symbol() {
    let tmp = project_of(&[(
        "scripts/run.sh",
        "#!/bin/sh\n# Launch the engine.\nexec engine --sky-secret \"$KEY\"\n",
    )]);
    let out = hint_in(&tmp, "sky-secret flag");
    assert!(
        out.lines().any(|l| l.starts_with("scripts/run.sh ")),
        "{out}"
    );
}

#[test]
fn hint_names_a_script_by_a_word_form_of_its_name() {
    let tmp = project_of(&[
        (
            "scripts/brew_provision.sh",
            "#!/bin/sh
# Arm a brew run.
provision_arm() {
  echo arm
}
provision_arm\n",
        ),
        (
            "scripts/brew.sh",
            "#!/bin/sh
# Run the brew.
brew_run() {
  echo run
}
brew_run\n",
        ),
    ]);
    // The real R2 prompt said "provisioning script"; the file is
    // brew_provision.sh. No prefix joins the two forms, the stem does.
    let out = hint_in(
        &tmp,
        "The provisioning script must refuse secrets on the command line",
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("scripts/brew_provision.sh")),
        "{out}"
    );
}

#[test]
fn hint_memory_line_takes_the_newer_of_two_equal_notes() {
    let tmp = project_of(&[
        (
            ".agents/memory/a.md",
            "---\ntitle: Aaa\nupdated: 2026-01-01\n---\n\nzebra rule.\n",
        ),
        (
            ".agents/memory/b.md",
            "---\ntitle: Bbb\nupdated: 2026-03-01\n---\n\nzebra rule.\n",
        ),
    ]);
    let out = hint_in(&tmp, "zebra please");
    assert!(memory_line(&out).unwrap().contains("b.md"), "{out}");
}

#[test]
fn hint_keeps_its_answer_when_the_prompt_names_many_files() {
    let tmp = project_of(&[
        ("a.rs", "pub fn first_thing() {}\n"),
        ("b.rs", "pub fn second_thing() {}\n"),
        ("c.rs", "pub fn third_thing() {}\n"),
        ("d.rs", "pub fn fourth_thing() {}\n"),
        ("e.rs", "pub fn fifth_thing() {}\n"),
        ("f.rs", "pub fn sixth_thing() {}\n"),
        ("target.rs", "pub fn parse_order_book() {}\n"),
    ]);
    let out = hint_in(
        &tmp,
        "a.rs b.rs c.rs d.rs e.rs f.rs all call parse_order_book",
    );
    assert!(out.contains("target.rs:1-1"), "{out}");
}

#[test]
fn hint_puts_first_the_line_whose_body_matches_the_prompt_by_stem() {
    let tmp = project_of(&[(
        "src/lib.rs",
        "pub fn first_vault() {\n    let keys = 1;\n}\n\n\
         /// Rotated credentials land in the vault.\npub fn second() {\n    rotate_keys();\n}\n",
    )]);
    let out = hint_in(&tmp, "rotation of keys in the vault credentials");
    let first = out.lines().nth(1).unwrap_or_default();
    assert!(first.contains("second"), "{out}");
}
