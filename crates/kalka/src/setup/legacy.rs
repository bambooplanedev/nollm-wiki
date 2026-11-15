//! The takeover: before writing its own entries, `kalka setup` removes what `wiki` 0.6.13 (this
//! tool's old name) wrote into the project, only where it is byte for byte the old tool's, and
//! lists the old-looking entries it leaves alone.

mod old;
mod report;

pub(super) use report::report;

use super::{claude, claude_local_mcp, edit, exclude_file, find_on_path, worktree_prefix};
use edit::{inside, other_names, Outcome};
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
    /// A listed server or hook the old tool's: an agent may still run the old server.
    pub running: bool,
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
            Outcome::Foreign(why) => {
                self.found.running |= what == "server" || what == "hook";
                self.found.differing.push(if why == LINK {
                    format!("{from}: reached through a link; the old wiki {what} there was left")
                } else {
                    format!("{from}: wiki {what} differs from what wiki wrote; remove it by hand")
                });
            }
        }
        Ok(())
    }

    /// `base/rel`'s text, after the link check; only a regular file with one name is read (a
    /// fifo would block, a hard link would edit its other names too). Only writes fail the
    /// takeover.
    fn probe(base: &Path, rel: &Path) -> std::result::Result<Option<String>, Left> {
        if !inside(base, rel) {
            return Err(Left::Link);
        }
        let path = base.join(rel);
        // A file where a folder should be (`.agents` a file) holds nothing either.
        let absent =
            |e: &std::io::Error| matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory);
        match std::fs::symlink_metadata(&path) {
            Err(e) if absent(&e) => return Ok(None),
            Ok(m) if m.is_file() && other_names(&m) => return Err(Left::Link),
            Ok(m) if m.is_file() => {}
            _ => return Err(Left::Unreadable),
        }
        match std::fs::read(path) {
            Ok(b) => String::from_utf8(b).map(Some).map_err(|_| Left::Unreadable),
            Err(e) if absent(&e) => Ok(None),
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
            // Read only, and only a regular UTF-8 file at the end of the link.
            Err(Left::Link)
                if std::fs::metadata(&path).is_ok_and(|m| m.is_file())
                    && std::fs::read(&path)
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                        .is_some_and(|t| old(Some(&t))) =>
            {
                Outcome::Foreign(LINK.into())
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
            // Every exact item (one line each), then another `wiki hint` item is listed.
            loop {
                let o = self.check(file, old_hook, |p, _| {
                    edit::remove_item(root, p, &parents, &item)
                })?;
                let (done, foreign) = (o == Outcome::Done, matches!(o, Outcome::Foreign(_)));
                self.note("hook", file, o)?;
                if done {
                    continue;
                }
                if !foreign
                    && Self::probe(root, Path::new(file)).is_ok_and(|t| old_hook(t.as_deref()))
                {
                    self.note("hook", file, Outcome::Foreign(String::new()))?;
                }
                break;
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

        // The lines local scope wrote for those folders (mod.rs:494, 545, 579-590; the two
        // `.claude/skills` ones by wiki before 0.6.0: the navigation skill until 0.4.0, the
        // memory skill until 0.5.0). One whose folder is still there (an edited file was left)
        // keeps ignoring it. The file is where setup puts its own lines; git resolved the way to
        // its git folder (outside the project in a linked worktree), so the link check covers
        // `info/exclude` below that.
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
        for dir in [
            ".agents/skills/wiki-navigation",
            ".pi/extensions/wiki-hint",
            ".claude/skills/wiki-navigation",
            ".claude/skills/wiki-memory",
        ] {
            let line = format!("/{prefix}{dir}/");
            // Not `exists()`: a dangling link is still the folder's place.
            if std::fs::symlink_metadata(root.join(dir)).is_ok() {
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
            // Judged the old tool's, but Claude Code did not remove it: listed, and setup goes on.
            Outcome::Foreign(why) if why.starts_with("`claude mcp remove` failed") => {
                let why = why.split_once(": ").map_or(why.as_str(), |(_, w)| w);
                t.found.running = true;
                t.found.differing.push(format!(
                    "claude (local scope): could not remove the old wiki server ({why}); run: claude mcp remove wiki --scope local"
                ));
            }
            Outcome::Foreign(_) => {
                // Listed only when Claude Code shows a `wiki` that runs `wiki` (no answer, or
                // another command, is nothing of the old tool's), named by the scope it shows;
                // its project scope is `.mcp.json`'s, listed once.
                let text = match claude(root, &["mcp", "get", "wiki"]) {
                    Ok((true, text)) if text.lines().any(|l| l.trim() == "Command: wiki") => text,
                    _ => return Ok(t.found),
                };
                let scope = text
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("Scope: "))
                    .and_then(|s| Some(s.split_whitespace().next()?.to_lowercase()));
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
