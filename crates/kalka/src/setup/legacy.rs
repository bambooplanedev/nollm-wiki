//! The takeover: before writing its own entries, `kalka setup` removes what `wiki` 0.6.13 (this
//! tool's old name) wrote into the project, only where it is byte for byte the old tool's, and
//! lists the old-looking entries it leaves alone.

use super::{claude_local_mcp, edit, exclude_file, find_on_path, worktree_prefix};
use crate::hash;
use edit::Outcome;
use serde_json::{json, Value};
use std::io::{ErrorKind, Result};
use std::path::Path;

/// Human-readable lines: what was removed (printed by `run`), and what only looks old.
#[derive(Default)]
pub(super) struct Found {
    pub removed: Vec<String>,
    // Reported by the next step of the takeover.
    #[allow(dead_code)]
    pub differing: Vec<String>,
}

const OLD_SKILL: &str = ".agents/skills/wiki-navigation/SKILL.md";
const OLD_HOOK: &str = ".pi/extensions/wiki-hint/index.ts";
const NAV_BEGIN: &str = "<!-- wiki:navigation -->\n";
const NAV_END: &str = "<!-- /wiki:navigation -->\n";

// BLAKE3 (`crate::hash`) hex digests of every text `wiki` 0.6.13 wrote or took for its own, built
// the way its `setup/mod.rs` built them from the files of `v0.6.13:wiki/src/setup/`.

/// The pi skill: `SKILL.md` with `<memory-dir>` set to `.agents/memory` (mod.rs:317), and each
/// `shipped/skill-N.md` that way or cut before `\n# Memory\n` (mod.rs:322-333). The variant with
/// the per-user memory folder (mod.rs:328) holds a machine's path, so it cannot be listed here: a
/// skill holding it is reported, not removed. Variants equal to another one are listed once.
const OLD_SKILL_DIGESTS: [&str; 11] = [
    "4317b9f0e72faaf2312662142a4d8c89a11d5d3669667e86c9cdad7dcead4953", // SKILL.md
    "b79037b38ed68393d12982cdbfdd66b14139a5136ac0d84cd8fca5f9720b52c9", // shipped/skill-1.md
    "28a164ccfae1f03d8a463d69b6f99c32640b70443570b85b5ff3a83d5a1d3543", // shipped/skill-2.md
    "ef03b30699c42cdd8fecfd8d0cb63cff1e721a86b55a2ea2cf9fd72d9daf0684", // shipped/skill-3.md
    "89ea720825f8fc32fa560a5e4ad3d60d0f79aca3a00dc8e47dec2a89dce99c06", // shipped/skill-4.md
    "4477eb068724310792501f487eb8a9ca01c52fb688a11a4c8e40213b8f08660c", // shipped/skill-5.md
    "5ed51420afb208078e7cd61762a908d9e8d112c6a76d52bae73265c4dfb37399", // shipped/skill-5.md, cut
    "db639e69510644935275f4bc9711db487f43a9df22a81f29f6e8fae55a02e981", // shipped/skill-6.md
    "70c3991d123f699290547e6e6f02befba73a5a5a418fa3cca3cbd3a21dfdf7dd", // shipped/skill-6.md, cut
    "a56b3152987b58e2e6e825c4e4d181da9314ff9bbb52e53b8ba6d54e2686a79f", // shipped/skill-7.md
    "449507b06bfb0e3fd37c6c7aa9d387e0d7c80ebf15fe1f028ad1906b902ff44e", // shipped/skill-8.md
];

/// The pi extension: `<cmd>` set to `wiki` (mod.rs:529-533).
const OLD_HOOK_DIGESTS: [&str; 2] = [
    "b9d49d3d23a6a0c1ebb7cfe4e85fd22fe2622d6b25fed285538afc2a1bee4a07", // wiki-hint.ts
    "9c2e8a391c38f3420a7e2ddf1089858d9263238664b0663da79a5354f72700e9", // shipped/wiki-hint-1.ts
];

/// The `AGENTS.md` block: `<!-- wiki:navigation -->\n`, the text, `<!-- /wiki:navigation -->\n`
/// (mod.rs:341-344, 422), the span the old `remove_block` looked for (edit.rs:261-263).
const OLD_BLOCK_DIGESTS: [&str; 3] = [
    "eadaad9c02e99861d27fd581a47a5739284d80cf830e426fc7ee85fab583f5c9", // navigation.md
    "74018e9acb5790edf573193892e6816c6853226eeb4e5f1d9f470ba85201ec76", // shipped/navigation-1.md
    "ac98c83130b3969252b397ab361edad3ebcf6b3d6f98a7cd944eb2c533a865a0", // shipped/navigation-2.md
];

/// The server entry, in every file and for Claude Code's CLI (mod.rs:388).
fn old_entry() -> Value {
    json!({"command": "wiki", "args": ["serve", "--source", "."]})
}

fn known(text: &str, digests: &[&str]) -> bool {
    digests.contains(&hash::to_hex(&hash::hash_str(text)).as_str())
}

/// `path`'s text; `None` when it is missing or not UTF-8 (no old text is either).
fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(b) => Ok(String::from_utf8(b).ok()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// From the first begin line through the end line after it: the bytes the digests cover.
fn block(text: &str) -> Option<&str> {
    let i = text.find(NAV_BEGIN)?;
    let j = text[i..].find(NAV_END)?;
    Some(&text[i..i + j + NAV_END.len()])
}

/// Drops every line equal to `line`; `true` when there was one.
fn remove_line(path: &Path, line: &str) -> Result<bool> {
    let Some(text) = read_text(path)? else {
        return Ok(false);
    };
    let kept: String = text
        .split_inclusive('\n')
        .filter(|l| l.lines().next() != Some(line))
        .collect();
    if kept.len() == text.len() {
        return Ok(false);
    }
    std::fs::write(path, kept)?;
    Ok(true)
}

impl Found {
    fn note(&mut self, what: &str, from: &str, o: Outcome) {
        match o {
            Outcome::Done => self
                .removed
                .push(format!("removed old wiki {what} from {from}")),
            Outcome::AlreadyThere => {}
            Outcome::Foreign(why) => self
                .differing
                .push(format!("{from}: old wiki {what}: {why}")),
        }
    }

    /// Everything but Claude Code's local scope, which needs its CLI.
    fn in_files(&mut self, root: &Path) -> Result<()> {
        let entry = old_entry();
        let mut pi = entry.clone();
        pi["exposure"] = json!("direct"); // mod.rs:458-461
        for (file, v) in [
            (".mcp.json", &entry),
            (".pi/mcp.json", &pi),
            (".cursor/mcp.json", &entry),
        ] {
            let path = root.join(file);
            let o = edit::remove_entry(
                root,
                &path,
                &["mcpServers"],
                "wiki",
                std::slice::from_ref(v),
            )?;
            self.note("server", file, o);
        }

        let item = json!({"hooks": [{"type": "command", "command": "wiki hint --claude-hook"}]}); // mod.rs:553
        for file in [".claude/settings.json", ".claude/settings.local.json"] {
            let path = root.join(file);
            let mut o = edit::remove_item(root, &path, &["hooks", "UserPromptSubmit"], &item)?;
            if o == Outcome::AlreadyThere
                && read_text(&path)?.is_some_and(|t| t.contains("wiki hint --claude-hook"))
            {
                o = Outcome::Foreign("a hook running `wiki hint --claude-hook` differs".into());
            }
            self.note("hook", file, o);
        }

        for (what, file, digests) in [
            ("skill", OLD_SKILL, &OLD_SKILL_DIGESTS[..]),
            ("extension", OLD_HOOK, &OLD_HOOK_DIGESTS[..]),
        ] {
            let path = root.join(file);
            let o = match read_text(&path)? {
                Some(t) if known(&t, digests) => edit::remove_file(root, &path, &[&t])?,
                None if !path.exists() => Outcome::AlreadyThere,
                _ => Outcome::Foreign("file has other content".into()),
            };
            self.note(what, file, o);
        }

        let path = root.join("AGENTS.md");
        let text = read_text(&path)?.unwrap_or_default();
        let o = match block(&text) {
            Some(b) if known(b, &OLD_BLOCK_DIGESTS) => edit::remove_block(root, &path, b)?,
            _ if text.lines().any(|l| l == NAV_BEGIN.trim_end()) => {
                Outcome::Foreign("the block has other content".into())
            }
            _ => Outcome::AlreadyThere,
        };
        self.note("navigation block", "AGENTS.md", o);

        // The lines local scope wrote for those two folders (mod.rs:494, 545, 579-590). One
        // whose folder is still there (an edited file was left) keeps ignoring it.
        if let Some(exclude) = exclude_file(root) {
            let prefix = worktree_prefix(root);
            for dir in [".agents/skills/wiki-navigation", ".pi/extensions/wiki-hint"] {
                let line = format!("/{prefix}{dir}/");
                if !root.join(dir).exists() && remove_line(&exclude, &line)? {
                    let what = format!("ignore line {line}");
                    self.note(&what, ".git/info/exclude", Outcome::Done);
                }
            }
        }
        Ok(())
    }
}

/// Runs before setup writes anything, in every location whatever `--scope` and `--agent` say.
pub(super) fn take_over(root: &Path) -> Result<Found> {
    let mut f = Found::default();
    f.in_files(root)?;
    // After `.mcp.json`, which Claude Code shows as a `wiki` of project scope. Only a local one
    // is removed; another scope's (user, project) is reported. Without the CLI there is nothing
    // to look through.
    if find_on_path("claude").is_some() {
        // Recognised as the old one did (mod.rs:243-259).
        let o = claude_local_mcp(root, "wiki", &[old_entry()], true);
        f.note("server", "claude (local scope)", o);
    }
    Ok(f)
}

#[cfg(test)]
mod tests;
