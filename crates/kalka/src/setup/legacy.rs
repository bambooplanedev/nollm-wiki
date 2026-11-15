//! The takeover: before writing its own entries, `kalka setup` removes what `wiki` 0.6.13 (this
//! tool's old name) wrote into the project, only where it is byte for byte the old tool's, and
//! lists the old-looking entries it leaves alone.

mod old;
mod report;

pub(super) use report::report;

use super::{claude, claude_local_mcp, edit, exclude_file, find_on_path, worktree_prefix};
use edit::Outcome;
use old::{
    block, known, old_block, old_entry, old_hook, old_server, NAV_BEGIN, OLD_BLOCK_DIGESTS,
    OLD_HOOK, OLD_HOOK_DIGESTS, OLD_SKILL, OLD_SKILL_DIGESTS,
};
use serde_json::json;
use std::io::{ErrorKind, Result, Write};
use std::path::{Path, PathBuf};

/// Human-readable lines: what was removed (each also printed as it happens), and what only looks
/// old.
#[derive(Default)]
pub(super) struct Found {
    pub removed: Vec<String>,
    pub differing: Vec<String>,
}

/// Why `base/rel` may not be touched: a link on the way below `base` (the project, or for the
/// exclude file the git folder git named), or a path that resolves outside `base`.
fn unsafe_path(base: &Path, rel: &Path) -> Option<&'static str> {
    let mut p = base.to_path_buf();
    for c in rel.components() {
        p.push(c);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => return Some("is a symbolic link; left"),
            Ok(_) => {}
            Err(_) => return None, // missing: nothing to remove
        }
    }
    match (p.canonicalize(), base.canonicalize()) {
        (Ok(a), Ok(b)) if a.starts_with(&b) => None,
        _ => Some("lies outside the project; left"),
    }
}

/// Drops every line of `text` equal to `line`; `None` when there is none.
fn without_line(text: &str, line: &str) -> Option<String> {
    let kept: String = text
        .split_inclusive('\n')
        .filter(|l| l.lines().next() != Some(line))
        .collect();
    (kept.len() != text.len()).then_some(kept)
}

/// Why a location is left without being read for removal.
enum Left {
    /// A link on the way, or a path out of the project.
    Link,
    /// Not readable as text (a folder, no permission, not UTF-8).
    Unreadable,
}

/// The `Foreign` reason that says a link was in the way.
const LINK: &str = "link";

struct Takeover<'a> {
    root: &'a Path,
    out: &'a mut dyn Write,
    found: Found,
}

impl Takeover<'_> {
    /// Prints a removal at once, so a later failure cannot hide it; a `Foreign` one is listed.
    fn note(&mut self, what: &str, from: &str, o: Outcome) -> Result<()> {
        match o {
            Outcome::Done => {
                let line = format!("removed old wiki {what} from {from}");
                writeln!(self.out, "{line}")?;
                self.found.removed.push(line);
            }
            Outcome::AlreadyThere => {}
            Outcome::Foreign(why) if why == LINK => self.found.differing.push(format!(
                "{from}: reached through a link; the old wiki {what} there was left"
            )),
            Outcome::Foreign(_) => self.found.differing.push(format!(
                "{from}: wiki {what} differs from what wiki wrote; remove it by hand"
            )),
        }
        Ok(())
    }

    /// `base/rel`'s text, after the link check. Only writes fail the takeover.
    fn probe(base: &Path, rel: &Path) -> std::result::Result<Option<String>, Left> {
        if unsafe_path(base, rel).is_some() {
            return Err(Left::Link);
        }
        match std::fs::read(base.join(rel)) {
            Ok(b) => String::from_utf8(b).map(Some).map_err(|_| Left::Unreadable),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(_) => Err(Left::Unreadable),
        }
    }

    /// Runs `remove` on `base/rel` when it can be read safely. What is left is `Foreign` only
    /// when `old` finds the old tool's thing in it, so a project without old traces stays
    /// silent; behind a link that is read, never written.
    fn left_or(
        base: &Path,
        rel: &Path,
        old: impl Fn(Option<&str>) -> bool,
        remove: impl FnOnce(&Path, String) -> Result<Outcome>,
    ) -> Result<Outcome> {
        let path = base.join(rel);
        Ok(match Self::probe(base, rel) {
            Ok(Some(t)) => match remove(&path, t.clone())? {
                Outcome::Foreign(_) if !old(Some(&t)) => Outcome::AlreadyThere,
                o => o,
            },
            Err(Left::Link) if path.exists() => {
                let t = std::fs::read(&path).ok();
                if old(t.as_deref().map(String::from_utf8_lossy).as_deref()) {
                    Outcome::Foreign(LINK.into())
                } else {
                    Outcome::AlreadyThere
                }
            }
            Err(Left::Unreadable) if old(None) => Outcome::Foreign(String::new()),
            Ok(None) | Err(_) => Outcome::AlreadyThere,
        })
    }

    fn check(
        &self,
        file: &str,
        old: impl Fn(Option<&str>) -> bool,
        remove: impl FnOnce(&Path, String) -> Result<Outcome>,
    ) -> Result<Outcome> {
        Self::left_or(self.root, Path::new(file), old, remove)
    }

    /// Everything but Claude Code's local scope, which needs its CLI.
    fn in_files(&mut self) -> Result<()> {
        let root = self.root;
        let entry = old_entry();
        let mut pi = entry.clone();
        pi["exposure"] = json!("direct"); // mod.rs:458-461
        for (file, v) in [
            (".mcp.json", &entry),
            (".pi/mcp.json", &pi),
            (".cursor/mcp.json", &entry),
        ] {
            let own = std::slice::from_ref(v);
            let o = self.check(file, old_server, |p, _| {
                edit::remove_entry(root, p, &["mcpServers"], "wiki", own)
            })?;
            self.note("server", file, o)?;
        }

        let item = json!({"hooks": [{"type": "command", "command": "wiki hint --claude-hook"}]}); // mod.rs:553
        for file in [".claude/settings.json", ".claude/settings.local.json"] {
            let parents = ["hooks", "UserPromptSubmit"];
            let o = self.check(file, old_hook, |p, _| {
                edit::remove_item(root, p, &parents, &item)
            })?;
            // Another `wiki hint` item, left beside the removed one or alone.
            let other = !matches!(o, Outcome::Foreign(_))
                && Self::probe(root, Path::new(file)).is_ok_and(|t| old_hook(t.as_deref()));
            self.note("hook", file, o)?;
            if other {
                self.note("hook", file, Outcome::Foreign(String::new()))?;
            }
        }

        for (what, file, digests) in [
            ("skill", OLD_SKILL, &OLD_SKILL_DIGESTS[..]),
            ("extension", OLD_HOOK, &OLD_HOOK_DIGESTS[..]),
        ] {
            // There at all, it is the old tool's place.
            let o = self.check(
                file,
                |_| true,
                |p, t| {
                    if known(&t, digests) {
                        edit::remove_file(root, p, &[&t])
                    } else {
                        Ok(Outcome::Foreign("file has other content".into()))
                    }
                },
            )?;
            self.note(what, file, o)?;
        }

        let o = self.check("AGENTS.md", old_block, |p, text| match block(&text) {
            Some(b) if known(b, &OLD_BLOCK_DIGESTS) => edit::remove_block(root, p, b),
            _ if text.lines().any(|l| l == NAV_BEGIN.trim_end()) => {
                Ok(Outcome::Foreign("the block has other content".into()))
            }
            _ => Ok(Outcome::AlreadyThere),
        })?;
        self.note("navigation block", "AGENTS.md", o)?;

        // The lines local scope wrote for those two folders (mod.rs:494, 545, 579-590). One
        // whose folder is still there (an edited file was left) keeps ignoring it. The file is
        // where setup puts its own lines; git resolved the way to its git folder (outside the
        // project in a linked worktree), so the link check covers `info/exclude` below that.
        let Some(exclude) = exclude_file(root) else {
            return Ok(());
        };
        let (base, rel): (PathBuf, PathBuf) = match exclude.strip_prefix(root) {
            Ok(rel) if !rel.starts_with("..") => (root.into(), rel.into()),
            _ => match exclude.parent().and_then(Path::parent) {
                Some(git_dir) => (git_dir.into(), "info/exclude".into()),
                None => return Ok(()),
            },
        };
        let prefix = worktree_prefix(root);
        for dir in [".agents/skills/wiki-navigation", ".pi/extensions/wiki-hint"] {
            let line = format!("/{prefix}{dir}/");
            if root.join(dir).exists() {
                continue;
            }
            let what = format!("ignore line {line}");
            let has = |t: Option<&str>| t.is_some_and(|t| t.lines().any(|l| l == line));
            let o = Self::left_or(&base, &rel, has, |_, text| {
                Ok(match without_line(&text, &line) {
                    Some(kept) => {
                        std::fs::write(&exclude, kept)?;
                        Outcome::Done
                    }
                    None => Outcome::AlreadyThere,
                })
            })?;
            self.note(&what, ".git/info/exclude", o)?;
        }
        Ok(())
    }
}

/// Runs before setup writes anything, in every location whatever `--scope` and `--agent` say;
/// prints each removal to `out` as it happens.
pub(super) fn take_over(root: &Path, out: &mut dyn Write) -> Result<Found> {
    let mut t = Takeover {
        root,
        out,
        found: Found::default(),
    };
    t.in_files()?;
    // After `.mcp.json`, which Claude Code shows as a `wiki` of project scope. Only a local one
    // is removed, and only with nothing but what the old `add-json` yields; another scope's
    // (user, project) is reported. Without the CLI there is nothing to look through.
    if find_on_path("claude").is_some() {
        // Recognised as the old one did (mod.rs:243-259), as strictly as kalka's own entry.
        match claude_local_mcp(root, "wiki", &[old_entry()], true, true) {
            Outcome::Foreign(_) => {
                // Named by the scope Claude Code shows; its project scope is `.mcp.json`'s,
                // listed once.
                let scope = claude(root, &["mcp", "get", "wiki"])
                    .ok()
                    .and_then(|(_, text)| {
                        let s = text
                            .lines()
                            .find_map(|l| l.trim().strip_prefix("Scope: "))?;
                        Some(s.split_whitespace().next()?.to_lowercase())
                    });
                let listed = |l: &String| l.starts_with(".mcp.json:");
                if !(scope.as_deref() == Some("project") && t.found.differing.iter().any(listed)) {
                    let from = scope.map_or("claude".into(), |s| format!("claude ({s} scope)"));
                    t.note("server", &from, Outcome::Foreign(String::new()))?;
                }
            }
            o => t.note("server", "claude (local scope)", o)?,
        }
    }
    Ok(t.found)
}

#[cfg(test)]
mod tests;
