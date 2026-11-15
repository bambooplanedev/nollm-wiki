//! The text side: the block, its exact recognition, and the edits. A mistake here may only ever
//! turn "append" into "refuse": any line that could define the server or an `mcp_servers` that a
//! `[mcp_servers.kalka]` table cannot join is read as doing so, whatever table it seems to be in.

use crate::setup::{serve_args, CMD};

const HEADER: &str = "[mcp_servers.kalka]";
/// Another definition of the server (the words of the old machine-wide report).
pub(super) const ANOTHER: &str = "another \"kalka\" server is configured";
/// An `mcp_servers` a `[mcp_servers.kalka]` table cannot be added to without breaking the file.
pub(super) const UNEXTENDABLE: &str =
    "the file defines mcp_servers inline, by dotted keys or as an array; add the kalka server by hand";
/// Refused, not stripped: the header behind it would not be found again.
pub(super) const BOM: &str = "starts with a byte order mark; left as it is";

fn command_line() -> String {
    format!("command = \"{CMD}\"")
}

fn args_line(memory: bool) -> String {
    format!("args = [\"{}\"]", serve_args(memory).join("\", \""))
}

pub(super) fn block(memory: bool, eol: &str) -> String {
    [HEADER, &command_line(), &args_line(memory)]
        .map(|l| format!("{l}{eol}"))
        .concat()
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum State {
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

/// What a squeezed header (`table`) or any other line beginning with `mcp_servers` says: the
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

pub(super) fn state(text: &str, memory: bool) -> State {
    if text.starts_with('\u{feff}') {
        return State::Other(BOM);
    }
    // No table is trusted for a refusal: a `[` line inside a multi-line string or array looks
    // like a header. Only the body of our own header is set apart, and it must then be exact.
    let (mut in_ours, mut servers_seen) = (false, false);
    let (mut headers, mut body, mut other) = (0, Vec::new(), None);
    for l in text.split_inclusive('\n').map(bare) {
        let t = l.trim();
        if t.is_empty() || (t.starts_with('#') && !in_ours) {
            continue;
        }
        let s = squeeze(t);
        if t == HEADER {
            headers += 1;
            (in_ours, servers_seen) = (true, true);
        } else if t.starts_with('[') {
            other = other.or(servers(&s, true));
            let key = s.trim_start_matches('[').strip_prefix("mcp_servers");
            servers_seen |= key.is_some_and(|rest| rest.starts_with([']', '.']));
            in_ours = false;
        } else if in_ours {
            body.push(l.trim_end());
        } else {
            other = other.or(servers(&s, false));
            // After any `[mcp_servers…]` header, a `kalka` key may be the server's.
            if servers_seen && names_kalka(&s) {
                other = other.or(Some(ANOTHER));
            }
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
pub(super) fn with_block(text: &str, memory: bool) -> String {
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
pub(super) fn with_args(text: &str, memory: bool) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let Some([_, _, a]) = block_lines(&lines) else {
        return text.into();
    };
    let eol = &lines[a][bare(lines[a]).len()..];
    let (before, after) = (lines[..a].concat(), lines[a + 1..].concat());
    format!("{before}{}{eol}{after}", args_line(memory))
}

/// `text` without an exact block and the one blank line before it.
pub(super) fn without_block(text: &str) -> String {
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
