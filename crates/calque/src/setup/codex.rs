//! Codex's MCP server, in the project's own `.codex/config.toml`. calque has no TOML library, so
//! its table is recognised as text, exactly; anything else that names the server, in any
//! spelling, is another server and the file is not touched.

mod toml;

use super::edit::{inside, other_names, prune_dirs, Outcome};
use super::{cli, Report};
use crate::{memory, WikiError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use toml::{state, with_args, with_block, without_block, State, ANOTHER};

pub(super) const FILE: &str = ".codex/config.toml";
const LINK: &str =
    "reached through a link, out of the project or a file with more than one name; left as it is";
const UNREADABLE: &str = "not a regular UTF-8 file; left as it is";
const HOME: &str =
    "this .codex/ is Codex's own (CODEX_HOME or ~/.codex), whose config is machine-wide; left as it is";
/// What to say after the project's block is written; `root` as setup knows it.
pub(super) fn hint(root: &Path) -> String {
    let path = memory::display_path(root);
    format!("Codex loads the server only in a project it trusts: run codex in the project and accept the question, or add [projects.\"{path}\"] trust_level = \"trusted\" to ~/.codex/config.toml")
}
/// What the machine-wide entry is reported as (`CODEX_HOME` may put it anywhere).
const MACHINE: &str = "Codex calque server (machine-wide)";

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

/// Whether Codex has a machine-wide `calque`, asked outside any project: inside one, Codex also
/// shows a trusted parent project's. Without `codex`, no.
fn machine_get() -> bool {
    cli("codex", &std::env::temp_dir(), &["mcp", "get", "calque"]).is_ok_and(|(found, _)| found)
}

/// After the project's block is in place: the machine-wide `calque` is another server that calque
/// did not write, reported (on `--remove` as a note: nothing of setup's was skipped).
fn machine_wide(r: &mut Report, remove: bool) -> Result<(), WikiError> {
    if remove {
        return r.line(&format!("left the {MACHINE}: another \"calque\" server that calque did not write; `codex mcp remove calque` removes it"));
    }
    r.skip(MACHINE, &format!("{ANOTHER} machine-wide and still runs in every other Codex session (`codex mcp remove calque` removes it); this project's {FILE} entry was written and takes precedence here"))
}

/// Writes (or with `remove`, takes out) the block; `true` when it is in place. The machine-wide
/// entry is looked at on every run that may edit the file, and reported only once the block is
/// in place: by setup on the run that finds the project without a `calque` of its own, and as a
/// note by every `--remove`.
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
    let machine = editable && machine_get();
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
    let present = r.file_item(root, FILE, &format!("{FILE} calque server"), edit)?;
    if machine && present && (absent || remove) {
        machine_wide(r, remove)?;
    }
    Ok(present)
}

#[cfg(test)]
mod tests;
