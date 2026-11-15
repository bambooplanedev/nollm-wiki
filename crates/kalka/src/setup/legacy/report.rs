//! What the old `wiki` left that the takeover does not change, as one block for the person or
//! the agent doing the install. Everything here only reads.

use super::old::{NAV_BEGIN, NAV_END};
use super::Found;
use crate::setup::{cli, exclude_file, find_on_path, git, worktree_prefix};
use serde_json::Value;
use std::path::Path;

/// Mentions listed before the rest is counted.
const MAX_MENTIONS: usize = 20;
/// A bigger instructions file is not read.
const MAX_SIZE: u64 = 1 << 20;

/// The block's lines (stderr), empty when there is nothing to say.
pub(in crate::setup) fn report(root: &Path, found: &Found) -> Vec<String> {
    let mut items: Vec<String> = found.differing.iter().map(|l| format!("  {l}")).collect();
    if codex_has_wiki(root) {
        items.push("  codex: server \"wiki\" is registered machine-wide; when no project uses it: codex mcp remove wiki".into());
    }
    if root.join(".wiki").is_dir() {
        items.push("  .wiki/: old compiled output (the new one is .kulman/); delete it when you no longer need it".into());
    }
    // As the old setup wrote it: `.wiki/` at the top, `/<prefix>.wiki/` below it (mod.rs:573-590).
    let ignore = [
        ".wiki/".to_string(),
        format!("/{}.wiki/", worktree_prefix(root)),
    ];
    for (file, path) in [
        (".gitignore", Some(root.join(".gitignore"))),
        (".git/info/exclude", exclude_file(root)),
    ] {
        let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else {
            continue;
        };
        for (i, l) in text.lines().enumerate() {
            if ignore.iter().any(|x| x == l) {
                items.push(format!("  {file}:{}: ignore line for .wiki/", i + 1));
            }
        }
    }
    if let Some(p) = find_on_path("wiki") {
        items.push(format!("  {}: an old wiki binary is on PATH", p.display()));
    }
    for file in [".claude/settings.json", ".claude/settings.local.json"] {
        items.extend(claude_settings(root, file));
    }
    items.extend(mentions(root));

    let mut lines = Vec::new();
    if !items.is_empty() {
        lines.push("kalka: left from the old wiki (not changed):".to_string());
        lines.append(&mut items);
        lines.push(
            "  rename: wiki -> kalka, mcp__wiki__* -> mcp__kalka__*, .wiki/ -> .kulman/".into(),
        );
    }
    if !lines.is_empty() || !found.removed.is_empty() {
        lines.push(
            "kalka: restart running agent sessions; they keep the old server until then".into(),
        );
    }
    lines
}

/// Codex's machine-wide `wiki`, read as `codex_mcp` reads its own entry (mod.rs:227-231), with
/// the old command and args. Never removed: other projects may still use it.
fn codex_has_wiki(root: &Path) -> bool {
    let Ok((true, text)) = cli("codex", root, &["mcp", "get", "wiki"]) else {
        return false;
    };
    let has = |l: &str| text.lines().any(|t| t.trim() == l);
    has("command: wiki") && has("args: serve --source .")
}

/// Settings Claude Code or the person wrote for the old server: permission rules and the
/// `.mcp.json` approvals.
fn claude_settings(root: &Path, file: &str) -> Vec<String> {
    let Some(v) = std::fs::read_to_string(root.join(file))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return Vec::new();
    };
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(String::from))
            .collect()
    };
    let mut out = Vec::new();
    let rules = ["allow", "deny", "ask"]
        .iter()
        .flat_map(|k| strings(&v["permissions"][k]));
    if rules
        .into_iter()
        .any(|r| r.starts_with("mcp__wiki__") || r == "mcp__wiki")
    {
        out.push(format!(
            "  {file}: permission rules name mcp__wiki__*; the same rules for mcp__kalka__* are needed"
        ));
    }
    for key in ["enabledMcpjsonServers", "disabledMcpjsonServers"] {
        if strings(&v[key]).iter().any(|s| s == "wiki") {
            out.push(format!("  {file}: {key} lists \"wiki\""));
        }
    }
    out
}

/// Lines of `CLAUDE.md` and `AGENTS.md`, at the top and one folder down, that name the old tool.
/// Links are not followed; the inside of a `wiki:navigation` block is the block's own line.
fn mentions(root: &Path) -> Vec<String> {
    let re = regex::Regex::new(
        r"mcp__wiki__|\.wiki/|\bwiki\s+(search|page|neighbors|callers|lint|compile|serve|setup|hint)\b",
    )
    .expect("a valid pattern");
    let mut dirs: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n != "node_modules" && n != "target")
        .collect();
    dirs.sort();
    let mut args = vec!["check-ignore", "--"];
    args.extend(dirs.iter().map(String::as_str));
    if let Some(ignored) = (!dirs.is_empty()).then(|| git(root, &args)).flatten() {
        let ignored: Vec<String> = ignored.lines().map(String::from).collect();
        dirs.retain(|d| !ignored.contains(d));
    }
    let files = [String::new()]
        .into_iter()
        .chain(dirs.into_iter().map(|d| format!("{d}/")))
        .flat_map(|d| [format!("{d}CLAUDE.md"), format!("{d}AGENTS.md")]);

    let mut hits = Vec::new();
    for file in files {
        let path = root.join(&file);
        let small_file = std::fs::symlink_metadata(&path)
            .is_ok_and(|m| m.file_type().is_file() && m.len() <= MAX_SIZE);
        let Some(text) = small_file
            .then(|| std::fs::read_to_string(&path).ok())
            .flatten()
        else {
            continue;
        };
        let mut inside = false;
        for (i, l) in text.lines().enumerate() {
            if l == NAV_BEGIN.trim_end() {
                inside = true;
            } else if l == NAV_END.trim_end() {
                inside = false;
            } else if !inside && re.is_match(l) {
                hits.push(format!("  {file}:{}: mentions the old tool", i + 1));
            }
        }
    }
    if hits.len() > MAX_MENTIONS {
        let more = hits.len() - MAX_MENTIONS;
        hits.truncate(MAX_MENTIONS);
        hits.push(format!("  … and {more} more"));
    }
    hits
}
