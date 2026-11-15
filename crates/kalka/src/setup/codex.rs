//! Codex's MCP server, in the project's own `.codex/config.toml`. kalka has no TOML library, so
//! its table is recognised as text, exactly; anything else that names the server, in any
//! spelling, is another server and the file is not touched.

use super::edit::{inside, other_names, prune_dirs, Outcome};
use super::{cli, serve_args, tracked, Report, CMD};
use crate::WikiError;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub(super) const FILE: &str = ".codex/config.toml";
const HEADER: &str = "[mcp_servers.kalka]";
/// Another definition of the server (the words of the old machine-wide report).
const ANOTHER: &str = "another \"kalka\" server is configured";
/// An `mcp_servers` a `[mcp_servers.kalka]` table cannot be added to without breaking the file.
const UNEXTENDABLE: &str =
    "the file defines mcp_servers inline, by dotted keys or as an array; add the kalka server by hand";
const LINK: &str =
    "reached through a link, out of the project or a file with more than one name; left as it is";
const UNREADABLE: &str = "not a regular UTF-8 file; left as it is";
const HOME: &str =
    "this .codex/ is Codex's own (CODEX_HOME or ~/.codex), whose config is machine-wide; left as it is";
pub(super) const HINT: &str = "Codex loads the server only in a project it trusts: open the project in Codex and accept the trust question.";
/// What the machine-wide entry is reported as, as the old setup named it.
const MACHINE: &str = "Codex kalka server (~/.codex/config.toml)";

fn command_line() -> String {
    format!("command = \"{CMD}\"")
}

fn args_line(memory: bool) -> String {
    format!("args = [\"{}\"]", serve_args(memory).join("\", \""))
}

fn block(memory: bool, eol: &str) -> String {
    [HEADER, &command_line(), &args_line(memory)]
        .map(|l| format!("{l}{eol}"))
        .concat()
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    /// The exact block, in this run's memory variant.
    Ours,
    /// The exact block, in the other one.
    OursOtherArgs,
    /// Anything else that defines the server, or an `mcp_servers` it cannot join.
    Other(&'static str),
    Absent,
}

/// A line without its `\n` or `\r\n`.
fn bare(l: &str) -> &str {
    l.strip_suffix('\n')
        .map_or(l, |l| l.strip_suffix('\r').unwrap_or(l))
}

/// The line's text without spaces, quotes and a byte order mark: `[ mcp_servers . "kalka" ]` reads `[mcp_servers.kalka]`.
fn squeeze(l: &str) -> String {
    l.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '\u{feff}'))
        .collect()
}

/// Whether a squeezed key starts with the key `kalka` (not `kalkax`).
fn names_kalka(key: &str) -> bool {
    let bare_key = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    key.strip_prefix("kalka")
        .is_some_and(|rest| !rest.starts_with(bare_key))
}

/// Indexes of the exact header and the two lines after it that are not blank.
fn block_lines(lines: &[&str]) -> Option<[usize; 3]> {
    let h = lines.iter().position(|l| bare(l).trim() == HEADER)?;
    let mut body = (h + 1..lines.len()).filter(|&i| !bare(lines[i]).trim().is_empty());
    Some([h, body.next()?, body.next()?])
}

/// What a squeezed header (`table`) or top-level line beginning with `mcp_servers` says: the
/// server defined, or an `mcp_servers` that a `[mcp_servers.kalka]` table cannot join.
fn servers(s: &str, table: bool) -> Option<&'static str> {
    let rest = s.trim_start_matches('[').strip_prefix("mcp_servers")?;
    if rest.strip_prefix('.').is_some_and(names_kalka)
        || (rest.starts_with('=') && s.contains("kalka"))
    {
        return Some(ANOTHER);
    }
    let joins = if table {
        s.starts_with("[[") && rest.starts_with(['.', ']'])
    } else {
        rest.starts_with(['.', '='])
    };
    joins.then_some(UNEXTENDABLE)
}

fn state(text: &str, memory: bool) -> State {
    // The squeezed key of the current table header (`None` at the top level); the first reason.
    let mut table: Option<String> = None;
    let (mut headers, mut body, mut other) = (0, Vec::new(), None);
    for l in text.split_inclusive('\n').map(bare) {
        let t = l.trim();
        let in_ours = table.as_deref() == Some(HEADER);
        if t.is_empty() || (t.starts_with('#') && !in_ours) {
            continue;
        }
        let s = squeeze(t);
        if t == HEADER {
            headers += 1;
            table = Some(HEADER.into());
        } else if t.starts_with('[') {
            other = other.or(servers(&s, true));
            table = Some(s.trim_start_matches('[').into());
        } else if in_ours {
            body.push(l.trim_end());
        } else if let Some(h) = &table {
            if h.starts_with("mcp_servers]") && names_kalka(&s) {
                other = other.or(Some(ANOTHER));
            }
        } else {
            other = other.or(servers(&s, false));
        }
    }
    if let Some(why) = other {
        return State::Other(why);
    }
    match (headers, body.as_slice()) {
        (0, _) => State::Absent,
        (1, [c, a]) if *c == command_line() => match *a {
            a if a == args_line(memory) => State::Ours,
            a if a == args_line(!memory) => State::OursOtherArgs,
            _ => State::Other(ANOTHER),
        },
        _ => State::Other(ANOTHER),
    }
}

/// `text` with the block at the end, after one blank line when there is text, in the ending of
/// the last line that has one (`\n` when none has).
fn with_block(text: &str, memory: bool) -> String {
    let eol = text
        .split_inclusive('\n')
        .rfind(|l| l.ends_with('\n'))
        .map_or("\n", |l| if l.ends_with("\r\n") { "\r\n" } else { "\n" });
    let mut out = text.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push_str(eol);
        }
        out.push_str(eol);
    }
    out + &block(memory, eol)
}

/// An exact block's `args` line in `memory`'s variant, its line ending kept.
fn with_args(text: &str, memory: bool) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let Some([_, _, a]) = block_lines(&lines) else {
        return text.into();
    };
    let eol = &lines[a][bare(lines[a]).len()..];
    let (before, after) = (lines[..a].concat(), lines[a + 1..].concat());
    format!("{before}{}{eol}{after}", args_line(memory))
}

/// `text` without an exact block and the one blank line before it.
fn without_block(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let Some([h, c, a]) = block_lines(&lines) else {
        return text.into();
    };
    let blank = h > 0 && bare(lines[h - 1]).trim().is_empty();
    let gone = |i: &usize| [h, c, a].contains(i) || (blank && i + 1 == h);
    (0..lines.len())
        .filter(|i| !gone(i))
        .map(|i| lines[i])
        .collect()
}

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
