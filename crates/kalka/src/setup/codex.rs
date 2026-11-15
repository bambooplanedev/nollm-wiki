//! Codex's MCP server, in the project's own `.codex/config.toml`. kalka has no TOML library, so
//! its table is recognised as text, exactly; anything else that names the server, in any
//! spelling, is another server and the file is not touched.

mod toml;

use super::edit::{inside, other_names, prune_dirs, Outcome};
use super::{cli, serve_args, Report, CMD};
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
pub(super) fn codex_home(root: &Path) -> bool {
    // Empty is unset.
    let home = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let h = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
            Some(Path::new(&h).join(".codex"))
        });
    // Resolved whole (`CODEX_HOME` may be a link); one that does not exist yet, by its parent.
    let real = |p: &Path| {
        let by_parent = || Some(p.parent()?.canonicalize().ok()?.join(p.file_name()?));
        p.canonicalize().ok().or_else(by_parent)
    };
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

/// The machine-wide `kalka`, asked outside any project: inside one, Codex also shows a trusted
/// parent project's. `Some(true)`: exactly the old setup's. Without `codex`, `None`.
fn machine_get() -> Option<bool> {
    match cli("codex", &std::env::temp_dir(), &["mcp", "get", "kalka"]) {
        Ok((true, get)) => Some(old_entry(&get)),
        _ => None,
    }
}

/// After the project's block is in place: the old setup's machine-wide entry is removed, another
/// one reported.
fn machine_wide(r: &mut Report, exact: bool, remove: bool) -> Result<(), WikiError> {
    if !exact {
        let why = if remove {
            format!("{ANOTHER} machine-wide and runs in every Codex session; `codex mcp remove kalka` removes it")
        } else {
            format!("{ANOTHER} machine-wide and still runs in every other Codex session (`codex mcp remove kalka` removes it); this project's {FILE} entry was written and takes precedence here")
        };
        return r.skip(MACHINE, &why);
    }
    match cli("codex", &std::env::temp_dir(), &["mcp", "remove", "kalka"]) {
        // Gone in the meantime: nothing was removed.
        Ok((_, out)) if out.contains("No MCP server named") => Ok(()),
        Ok(_) => {
            r.line("removed the machine-wide Codex kalka server (it ran in every Codex session)")
        }
        Err(e) => r.skip(
            MACHINE,
            &format!("{e}; run `codex mcp remove kalka` by hand"),
        ),
    }
}

/// Writes (or with `remove`, takes out) the block; `true` when it is in place. The machine-wide
/// entry is looked at on every run that may edit the file, and acted on only once the block is
/// in place (written, already there, removed, or nothing to remove), so a failed write never
/// leaves no server at all: an exact one goes (a failed remove is retried next run), another is
/// reported on the run that finds the project without a `kalka` of its own.
pub(super) fn run(
    root: &Path,
    r: &mut Report,
    remove: bool,
    memory: bool,
) -> Result<bool, WikiError> {
    let text = read(root);
    let st = |t: &Option<String>| state(t.as_deref().unwrap_or_default(), memory);
    let absent = text.as_ref().is_ok_and(|t| st(t) == State::Absent);
    let editable = text
        .as_ref()
        .is_ok_and(|t| !matches!(st(t), State::Other(_)));
    let machine = editable.then(machine_get).flatten();
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
    let present = r.file_item(root, FILE, &format!("{FILE} kalka server"), false, edit)?;
    if let Some(exact) = machine.filter(|&exact| present && (exact || absent)) {
        machine_wide(r, exact, remove)?;
    }
    Ok(present)
}

#[cfg(test)]
mod tests;
