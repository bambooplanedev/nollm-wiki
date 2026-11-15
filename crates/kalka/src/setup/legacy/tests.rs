//! Tests for `legacy`: what is left and reported. The removals are in `tests/setup_takeover.rs`.
use super::*;
use std::fs;

fn found_in(files: &[(&str, &str)]) -> (tempfile::TempDir, Found) {
    let d = tempfile::tempdir().unwrap();
    for (file, text) in files {
        let p = d.path().join(file);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    let mut out = Vec::new();
    let mut t = Takeover {
        root: d.path(),
        out: &mut out,
        found: Found::default(),
    };
    t.in_files().unwrap();
    let f = t.found;
    let printed: Vec<String> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(printed, f.removed);
    (d, f)
}

#[test]
fn edited_old_entries_are_reported_and_left() {
    let skill = "---\nname: wiki-navigation\n---\nmine\n";
    let mcp = "{\"mcpServers\":{\"wiki\":{\"command\":\"wiki\",\"args\":[\"serve\",\"--source\",\"/abs\"]}}}";
    let hook = "{\"hooks\":{\"UserPromptSubmit\":[{\"matcher\":\"x\",\"hooks\":[{\"type\":\"command\",\"command\":\"wiki hint --claude-hook\"}]}]}}";
    let agents = "<!-- wiki:navigation -->\nmy steps\n<!-- /wiki:navigation -->\n";
    let files = [
        (OLD_SKILL, skill),
        (".mcp.json", mcp),
        (".claude/settings.json", hook),
        ("AGENTS.md", agents),
        (".git/info/exclude", "/.agents/skills/wiki-navigation/\n"),
    ];
    let (d, f) = found_in(&files);
    assert_eq!(f.removed, Vec::<String>::new());
    assert_eq!(
        f.differing,
        [
            ".mcp.json: wiki server differs from what wiki wrote; remove it by hand",
            ".claude/settings.json: wiki hook differs from what wiki wrote; remove it by hand",
            ".agents/skills/wiki-navigation/SKILL.md: wiki skill differs from what wiki wrote; remove it by hand",
            "AGENTS.md: wiki navigation block differs from what wiki wrote; remove it by hand",
        ]
    );
    for (file, text) in files {
        assert_eq!(
            fs::read_to_string(d.path().join(file)).unwrap(),
            text,
            "{file}"
        );
    }
}

/// No server `wiki` can be read from it, so it says nothing; setup meets it as it did before.
#[test]
fn a_config_that_is_not_a_json_object_is_left_silently() {
    let bad = "{ \"wiki\": not json";
    let (d, f) = found_in(&[(".mcp.json", bad), (".cursor/mcp.json", "[1]")]);
    assert!(f.removed.is_empty() && f.differing.is_empty());
    assert_eq!(fs::read_to_string(d.path().join(".mcp.json")).unwrap(), bad);
}

#[test]
fn every_exact_hook_item_goes_in_one_run() {
    let item = r#"{"hooks":[{"type":"command","command":"wiki hint --claude-hook"}]}"#;
    let hooks = format!(r#"{{"hooks":{{"UserPromptSubmit":[{item},{item}]}}}}"#);
    let (d, f) = found_in(&[(".claude/settings.json", &hooks)]);
    let line = "removed old wiki hook from .claude/settings.json";
    assert_eq!(f.removed, [line, line]);
    assert!(f.differing.is_empty(), "{:?}", f.differing);
    assert!(!d.path().join(".claude/settings.json").exists());
}

#[test]
fn a_permission_rule_naming_the_hook_is_no_hook() {
    let rule = r#"{"permissions":{"allow":["Bash(wiki hint --claude-hook)"]}}"#;
    let (_d, f) = found_in(&[(".claude/settings.json", rule)]);
    assert!(
        f.removed.is_empty() && f.differing.is_empty(),
        "{:?}",
        f.differing
    );
}

#[test]
fn nothing_old_finds_nothing() {
    let (_d, f) = found_in(&[
        (
            ".mcp.json",
            "{\"mcpServers\":{\"kalka\":{\"command\":\"kalka\"}}}",
        ),
        ("AGENTS.md", "# Team\n"),
        (".git/info/exclude", ".wiki/\n/.agents/memory/\n"),
    ]);
    assert!(f.removed.is_empty() && f.differing.is_empty());
}

#[test]
fn the_block_is_hashed_from_its_begin_line_through_its_end_line() {
    let text = "a\n<!-- wiki:navigation -->\nx\n<!-- /wiki:navigation -->\nb\n";
    assert_eq!(
        block(text),
        Some("<!-- wiki:navigation -->\nx\n<!-- /wiki:navigation -->\n")
    );
    assert_eq!(block("<!-- wiki:navigation -->\nx\n"), None);
}

#[test]
fn without_line_drops_only_the_exact_line() {
    let kept = without_line("/a/\n/a/b/\n/a/\r\n/a/", "/a/");
    assert_eq!(kept.as_deref(), Some("/a/b/\n"));
    assert_eq!(without_line("/a/b/\n", "/a/"), None);
}

#[test]
fn a_differing_hook_beside_a_removed_one_is_reported() {
    let hooks = r#"{"hooks":{"UserPromptSubmit":[
        {"hooks":[{"type":"command","command":"wiki hint --claude-hook"}]},
        {"matcher":"x","hooks":[{"type":"command","command":"wiki hint --claude-hook"}]}]}}"#;
    let (d, f) = found_in(&[(".claude/settings.local.json", hooks)]);
    assert_eq!(
        f.removed,
        ["removed old wiki hook from .claude/settings.local.json"]
    );
    assert_eq!(
        f.differing,
        [".claude/settings.local.json: wiki hook differs from what wiki wrote; remove it by hand"]
    );
    let left = fs::read_to_string(d.path().join(".claude/settings.local.json")).unwrap();
    assert!(left.contains("\"matcher\": \"x\""), "{left}");
}

#[test]
fn an_unreadable_location_is_listed_only_where_the_old_tool_wrote_a_file() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join(".mcp.json")).unwrap();
    fs::create_dir_all(d.path().join(OLD_HOOK)).unwrap();
    let mut out = Vec::new();
    let mut t = Takeover {
        root: d.path(),
        out: &mut out,
        found: Found::default(),
    };
    t.in_files().unwrap();
    assert_eq!(
        t.found.differing,
        [".pi/extensions/wiki-hint/index.ts: wiki extension differs from what wiki wrote; remove it by hand"]
    );
}
