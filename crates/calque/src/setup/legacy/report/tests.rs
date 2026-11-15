//! Tests for `report`'s read-only scans. Codex and the PATH need the binary: `setup_takeover.rs`.
use super::*;
use std::fs;

fn dir(files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (file, text) in files {
        let p = d.path().join(file);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    d
}

#[test]
fn settings_lines_need_the_old_names() {
    let d = dir(&[
        (
            ".claude/settings.json",
            br#"{"permissions":{"allow":["mcp__wikipedia__x"]},"enabledMcpjsonServers":["wikipedia"]}"#,
        ),
        (
            ".claude/settings.local.json",
            br#"{"permissions":{"allow":["mcp__wiki__search","mcp__calque__search"]},"disabledMcpjsonServers":["wiki"]}"#,
        ),
    ]);
    assert_eq!(
        claude_settings(d.path(), ".claude/settings.json"),
        Vec::<String>::new()
    );
    // A calque rule already there: the switch is made; the approval is still listed.
    assert_eq!(
        claude_settings(d.path(), ".claude/settings.local.json"),
        ["  .claude/settings.local.json: disabledMcpjsonServers lists \"wiki\""]
    );
}

#[test]
fn mentions_read_only_what_they_should() {
    let big = format!("wiki search\n{}", "x".repeat(1 << 20));
    let block = "<!-- wiki:navigation -->\nrun `wiki search` first\n<!-- /wiki:navigation -->\n";
    let d = dir(&[
        ("CLAUDE.md", format!("{block}after: wiki page\n").as_bytes()),
        // An open marker hides nothing.
        ("AGENTS.md", b"<!-- wiki:navigation -->\nwiki page\n"),
        (".claude/CLAUDE.md", b"see .wiki/index.md\n"),
        ("docs/CLAUDE.md", b"caf\xe9: wiki hint\n"),
        ("big/CLAUDE.md", big.as_bytes()),
        ("node_modules/CLAUDE.md", b"wiki page\n"),
        (".other/CLAUDE.md", b"wiki page\n"),
        ("elsewhere.md", b"wiki page\n"),
    ]);
    // A linked file is not followed.
    fs::create_dir_all(d.path().join("lib")).unwrap();
    let (target, link) = (
        d.path().join("elsewhere.md"),
        d.path().join("lib/CLAUDE.md"),
    );
    std::os::unix::fs::symlink(target, link).unwrap();
    assert_eq!(
        mentions(d.path()),
        [
            "  CLAUDE.md:4: mentions the old tool",
            "  AGENTS.md:2: mentions the old tool",
            "  docs/CLAUDE.md:1: mentions the old tool",
            "  .claude/CLAUDE.md:1: mentions the old tool",
        ]
    );
}

#[test]
fn mentions_catch_the_name_in_backticks_and_not_plain_prose() {
    let text = "use the `wiki` MCP first\n\
                run `wiki search` now\n\
                run `wiki` search now\n\
                the wiki MCP\n\
                wiki pages\n\
                our wiki\n\
                `wikipedia` MCP\n\
                `wiki` pages\n\
                the `wiki` is old\n";
    let d = dir(&[("CLAUDE.md", text.as_bytes())]);
    assert_eq!(
        mentions(d.path()),
        [1, 2, 3].map(|n| format!("  CLAUDE.md:{n}: mentions the old tool"))
    );
}

#[test]
fn mentions_skip_a_folder_git_ignores() {
    assert!(
        crate::setup::find_on_path("git").is_some(),
        "needs git on PATH"
    );
    let d = dir(&[
        (".gitignore", b"vendor/\n"),
        ("vendor/CLAUDE.md", b"wiki page\n"),
        ("src/CLAUDE.md", b"wiki page\n"),
    ]);
    let ok = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(d.path())
        .status()
        .unwrap();
    assert!(ok.success());
    assert_eq!(
        mentions(d.path()),
        ["  src/CLAUDE.md:1: mentions the old tool"]
    );
}
