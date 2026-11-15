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
    let mut f = Found::default();
    f.in_files(d.path()).unwrap();
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
            ".mcp.json: old wiki server: \"wiki\" has another value",
            ".claude/settings.json: old wiki hook: a hook running `wiki hint --claude-hook` differs",
            ".agents/skills/wiki-navigation/SKILL.md: old wiki skill: file has other content",
            "AGENTS.md: old wiki navigation block: the block has other content",
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

#[test]
fn a_config_that_is_not_a_json_object_is_reported() {
    let (d, f) = found_in(&[(".mcp.json", "{ not json"), (".cursor/mcp.json", "[1]")]);
    assert_eq!(
        f.differing,
        [
            ".mcp.json: old wiki server: not a JSON object",
            ".cursor/mcp.json: old wiki server: not a JSON object",
        ]
    );
    assert_eq!(
        fs::read_to_string(d.path().join(".mcp.json")).unwrap(),
        "{ not json"
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
fn remove_line_drops_only_the_exact_line() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("exclude");
    fs::write(&p, "/a/\n/a/b/\n/a/\r\n/a/").unwrap();
    assert!(remove_line(&p, "/a/").unwrap());
    assert_eq!(fs::read_to_string(&p).unwrap(), "/a/b/\n");
    assert!(!remove_line(&p, "/a/").unwrap());
}
