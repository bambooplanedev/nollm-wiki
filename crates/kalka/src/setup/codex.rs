//! Codex's MCP server, in the project's own `.codex/config.toml`. kalka has no TOML library, so
//! its table is recognised as text, exactly; anything else that names the server, in any
//! spelling, is another server and the file is not touched.

mod toml;

use super::edit::{inside, other_names, prune_dirs, Outcome};
use super::{cli, serve_args, tracked, Report, CMD};
use crate::WikiError;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use toml::{state, with_args, with_block, without_block, State, ANOTHER};

pub(super) const FILE: &str = ".codex/config.toml";
const LINK: &str =
    "reached through a link, out of the project or a file with more than one name; left as it is";
const UNREADABLE: &str = "not a regular UTF-8 file; left as it is";
const HOME: &str =
    "this .codex/ is Codex's own (CODEX_HOME or ~/.codex), whose config is machine-wide; left as it is";
pub(super) const HINT: &str = "Codex loads the server only in a project it trusts: open the project in Codex and accept the trust question.";
/// What the machine-wide entry is reported as, as the old setup named it.
const MACHINE: &str = "Codex kalka server (~/.codex/config.toml)";

/// The file's text (`None`: absent) when it may be edited: no link on the way, inside the
/// project, a regular file with one name, UTF-8; not Codex's own home.
fn read(root: &Path) -> Result<Option<String>, &'static str> {
    if !inside(root, Path::new(FILE)) {
        return Err(LINK);
    }
    if codex_home(root) {
        return Err(HOME);
    }
    let path = root.join(FILE);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Ok(m) if m.is_file() && other_names(&m) => return Err(LINK),
        Ok(m) if m.is_file() => {}
        _ => return Err(UNREADABLE),
    }
    let text = std::fs::read(&path)
        .ok()
        .and_then(|b| String::from_utf8(b).ok());
    text.map(Some).ok_or(UNREADABLE)
}

/// Whether `root/.codex` is where Codex keeps its machine-wide config.
fn codex_home(root: &Path) -> bool {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let h = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
            Some(Path::new(&h).join(".codex"))
        });
    // The folder may not exist yet: its parent is resolved.
    let real = |p: &Path| Some(p.parent()?.canonicalize().ok()?.join(p.file_name()?));
    home.is_some_and(|h| real(&h).is_some_and(|h| Some(h) == real(&root.join(".codex"))))
}

/// The entry the old setup's `codex mcp add` wrote, in either memory variant, and nothing else.
fn old_entry(get: &str) -> bool {
    let cmd = format!("command: {CMD}");
    let args = [true, false].map(|m| format!("args: {}", serve_args(m).join(" ")));
    let own = [
        &cmd,
        "kalka",
        "enabled: true",
        "transport: stdio",
        "cwd: -",
        "env: -",
    ];
    let lines = || get.lines().map(str::trim).filter(|l| !l.is_empty());
    let ours = |l: &str| own.contains(&l) || args.iter().any(|a| a == l);
    lines().any(|l| l == cmd)
        && lines().any(|l| args.iter().any(|a| a == l))
        && lines().all(|l| ours(l) || l == "remove: codex mcp remove kalka")
}

/// The old machine-wide entry (Codex shows it while the project defines no `kalka`): kalka's own
/// is removed, another one reported. Without `codex` there is nothing to ask.
fn machine_wide(root: &Path, r: &mut Report, remove: bool) -> Result<(), WikiError> {
    let Ok((true, get)) = cli("codex", root, &["mcp", "get", "kalka"]) else {
        return Ok(());
    };
    if !old_entry(&get) {
        let here = format!("; this project's {FILE} entry overrides it here");
        return r.skip(
            MACHINE,
            &format!("{ANOTHER}{}", if remove { "" } else { &here }),
        );
    }
    match cli("codex", root, &["mcp", "remove", "kalka"]) {
        Ok(_) => {
            r.line("removed the machine-wide Codex kalka server (it ran in every Codex session)")
        }
        Err(e) => r.skip(
            MACHINE,
            &format!("{e}; run `codex mcp remove kalka` by hand"),
        ),
    }
}

/// Writes (or with `remove`, takes out) the block; `true` when it is in place.
pub(super) fn run(
    root: &Path,
    r: &mut Report,
    remove: bool,
    memory: bool,
) -> Result<bool, WikiError> {
    let text = read(root);
    let st = |t: &Option<String>| state(t.as_deref().unwrap_or_default(), memory);
    // Only where this run goes on to write: the project's entry would hide the machine-wide one.
    let writes = !(r.guard_tracked && tracked(root, FILE));
    if text.as_ref().is_ok_and(|t| st(t) == State::Absent) && writes {
        machine_wide(root, r, remove)?;
    }
    let path = root.join(FILE);
    let edit = || -> std::io::Result<Outcome> {
        let text = match text {
            Ok(t) => t,
            Err(why) => return Ok(Outcome::Foreign(why.into())),
        };
        let cur = text.as_deref().unwrap_or_default();
        let new = match (st(&text), remove) {
            (State::Other(why), _) => return Ok(Outcome::Foreign(why.into())),
            (State::Absent, true) | (State::Ours, false) => return Ok(Outcome::AlreadyThere),
            (State::Absent, false) => with_block(cur, memory),
            (State::OursOtherArgs, false) => with_args(cur, memory),
            (_, true) => without_block(cur),
        };
        if new.trim().is_empty() {
            std::fs::remove_file(&path)?;
            prune_dirs(root, &path);
        } else {
            std::fs::create_dir_all(root.join(".codex"))?;
            std::fs::write(&path, new)?;
        }
        Ok(Outcome::Done)
    };
    r.file_item(root, FILE, &format!("{FILE} kalka server"), false, edit)
}

#[cfg(test)]
mod tests;
