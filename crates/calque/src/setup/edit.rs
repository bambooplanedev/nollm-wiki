//! Owned edits: every write is recognisable by exact value or content, so it can be undone without touching anything else.

use serde_json::{Map, Value};
use std::io::{ErrorKind, Result};
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Written or removed.
    Done,
    /// Nothing to do: present on install, absent on remove.
    AlreadyThere,
    /// Something else occupies the place; the reason is for the user.
    Foreign(String),
}

fn foreign(why: &str) -> Outcome {
    Outcome::Foreign(why.to_string())
}

fn read_opt(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn create_parent(path: &Path) -> Result<()> {
    path.parent().map_or(Ok(()), std::fs::create_dir_all)
}

/// Runs `f` on the object found under `parents` (created when `create`), then prunes emptied
/// parents and writes the file back, or deletes it when the root ends empty.
fn edit(
    path: &Path,
    parents: &[&str],
    create: bool,
    f: impl FnOnce(&mut Map<String, Value>) -> Outcome,
) -> Result<Outcome> {
    let text = read_opt(path)?;
    if text.is_none() && !create {
        return Ok(Outcome::AlreadyThere);
    }
    let mut root = match text.as_deref().map(serde_json::from_str::<Value>) {
        None => Map::new(),
        Some(Ok(Value::Object(m))) => m,
        Some(_) => return Ok(foreign("not a JSON object")),
    };
    let mut cur = &mut root;
    for p in parents {
        let next = if create {
            cur.entry(*p).or_insert_with(|| Value::Object(Map::new()))
        } else if let Some(v) = cur.get_mut(*p) {
            v
        } else {
            return Ok(Outcome::AlreadyThere);
        };
        let Value::Object(m) = next else {
            return Ok(foreign(&format!("\"{p}\" is not a JSON object")));
        };
        cur = m;
    }
    let out = f(cur);
    if out != Outcome::Done {
        return Ok(out);
    }
    prune(&mut root, parents);
    if root.is_empty() {
        std::fs::remove_file(path)?;
    } else {
        create_parent(path)?;
        let mut s = serde_json::to_string_pretty(&root).map_err(std::io::Error::from)?;
        s.push('\n');
        std::fs::write(path, s)?;
    }
    Ok(Outcome::Done)
}

/// Drops the objects along `parents` that ended up empty, deepest first.
fn prune(map: &mut Map<String, Value>, parents: &[&str]) {
    let Some((first, rest)) = parents.split_first() else {
        return;
    };
    if let Some(Value::Object(child)) = map.get_mut(*first) {
        prune(child, rest);
        if child.is_empty() {
            map.shift_remove(*first);
        }
    }
}

/// Sets `key` to `own[0]`; the other values in `own` are calque's too and are replaced in place.
pub fn upsert_entry(path: &Path, parents: &[&str], key: &str, own: &[Value]) -> Result<Outcome> {
    edit(path, parents, true, |m| match m.get(key) {
        Some(v) if *v == own[0] => Outcome::AlreadyThere,
        Some(v) if !own.contains(v) => Outcome::Foreign(format!("\"{key}\" has another value")),
        _ => {
            m.insert(key.to_string(), own[0].clone());
            Outcome::Done
        }
    })
}

/// Removes the directories under `root` that `path`'s removal left empty, deepest first.
pub(super) fn prune_dirs(root: &Path, path: &Path) {
    if !path.starts_with(root) {
        return;
    }
    for dir in path.ancestors().skip(1).take_while(|d| *d != root) {
        // `remove_dir` fails on a non-empty directory, which is the stop condition.
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

/// Removes `key` when it holds one of the `own` values. Deletes the file too (and the directories
/// it leaves empty, up to `root`) when the entry was its last content.
pub fn remove_entry(
    root: &Path,
    path: &Path,
    parents: &[&str],
    key: &str,
    own: &[Value],
) -> Result<Outcome> {
    let out = edit(path, parents, false, |m| match m.get(key) {
        None => Outcome::AlreadyThere,
        Some(v) if own.contains(v) => {
            m.shift_remove(key);
            Outcome::Done
        }
        Some(_) => Outcome::Foreign(format!("\"{key}\" has another value")),
    })?;
    if out == Outcome::Done && !path.exists() {
        prune_dirs(root, path);
    }
    Ok(out)
}

/// `parents` ends at the array's key: `["hooks", "UserPromptSubmit"]` is the array `hooks.UserPromptSubmit`.
/// An array left empty is dropped.
fn item_edit(
    path: &Path,
    parents: &[&str],
    create: bool,
    f: impl FnOnce(&mut Vec<Value>) -> Outcome,
) -> Result<Outcome> {
    let (key, up) = parents.split_last().expect("an array path is not empty");
    edit(path, up, create, |m| {
        let slot = if create {
            m.entry(*key).or_insert_with(|| Value::Array(Vec::new()))
        } else if let Some(v) = m.get_mut(*key) {
            v
        } else {
            return Outcome::AlreadyThere;
        };
        let Value::Array(items) = slot else {
            return Outcome::Foreign(format!("\"{key}\" is not an array"));
        };
        let out = f(items);
        if items.is_empty() {
            m.shift_remove(*key);
        }
        out
    })
}

/// Appends `own[0]`, or puts it in the place of one of the other `own` items.
pub fn append_item(path: &Path, parents: &[&str], own: &[Value]) -> Result<Outcome> {
    item_edit(path, parents, true, |items| {
        if items.contains(&own[0]) {
            return Outcome::AlreadyThere;
        }
        match items.iter().position(|v| own.contains(v)) {
            Some(i) => items[i] = own[0].clone(),
            None => items.push(own[0].clone()),
        }
        Outcome::Done
    })
}

/// Like [`remove_entry`], for an array item: every one of the `own` items found.
pub fn remove_item(root: &Path, path: &Path, parents: &[&str], own: &[Value]) -> Result<Outcome> {
    let out = item_edit(path, parents, false, |items| {
        let n = items.len();
        items.retain(|v| !own.contains(v));
        if items.len() < n {
            Outcome::Done
        } else {
            Outcome::AlreadyThere
        }
    })?;
    if out == Outcome::Done && !path.exists() {
        prune_dirs(root, path);
    }
    Ok(out)
}

/// Writes `own[0]`, over one of the other `own` contents; any other content already there is foreign.
pub fn write_file(path: &Path, own: &[&str]) -> Result<Outcome> {
    match read_opt(path)? {
        Some(s) if s == own[0] => Ok(Outcome::AlreadyThere),
        Some(s) if !own.contains(&s.as_str()) => Ok(foreign("file has other content")),
        _ => {
            create_parent(path)?;
            std::fs::write(path, own[0])?;
            Ok(Outcome::Done)
        }
    }
}

/// Removes `path` when it holds exactly one of the `own` contents, then the parent directories it leaves empty, up to but excluding `root`.
pub fn remove_file(root: &Path, path: &Path, own: &[&str]) -> Result<Outcome> {
    match read_opt(path)? {
        None => return Ok(Outcome::AlreadyThere),
        Some(s) if !own.contains(&s.as_str()) => return Ok(foreign("file has other content")),
        Some(_) => {}
    }
    std::fs::remove_file(path)?;
    prune_dirs(root, path);
    Ok(Outcome::Done)
}

/// Adds `block` (which starts with the line `begin`) to a text file, after a blank line when the
/// file has other content. A file holding another block that starts with `begin` is foreign.
pub fn upsert_block(path: &Path, begin: &str, block: &str) -> Result<Outcome> {
    let mut text = read_opt(path)?.unwrap_or_default();
    if text.contains(block) {
        return Ok(Outcome::AlreadyThere);
    }
    if text.lines().any(|l| l == begin) {
        return Ok(foreign("holds another calque block"));
    }
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(block);
    create_parent(path)?;
    std::fs::write(path, text)?;
    Ok(Outcome::Done)
}

/// Takes out `block` and the blank line [`upsert_block`] put before it; deletes the file (and the directories it leaves empty) when
/// nothing else is left. Another block under the same begin line is foreign.
pub fn remove_block(root: &Path, path: &Path, block: &str) -> Result<Outcome> {
    let Some(text) = read_opt(path)? else {
        return Ok(Outcome::AlreadyThere);
    };
    if !text.contains(block) {
        let begin = block.lines().next().unwrap_or_default();
        return Ok(if text.lines().any(|l| l == begin) {
            foreign("holds another calque block")
        } else {
            Outcome::AlreadyThere
        });
    }
    let rest = if let Some(i) = text.find(&format!("\n\n{block}")) {
        format!("{}{}", &text[..=i], &text[i + 2 + block.len()..])
    } else {
        let i = text.find(block).expect("found above");
        format!("{}{}", &text[..i], &text[i + block.len()..])
    };
    if rest.trim().is_empty() {
        std::fs::remove_file(path)?;
        prune_dirs(root, path);
    } else {
        std::fs::write(path, rest)?;
    }
    Ok(Outcome::Done)
}

pub fn ensure_line(path: &Path, line: &str) -> Result<Outcome> {
    let mut text = read_opt(path)?.unwrap_or_default();
    if text.lines().any(|l| l == line) {
        return Ok(Outcome::AlreadyThere);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    create_parent(path)?;
    std::fs::write(path, text)?;
    Ok(Outcome::Done)
}

/// Whether `base/rel` may be touched: no link on the way below `base` (dangling ones count),
/// and, when it exists, it resolves inside `base`. Missing is fine: there is nothing to edit.
pub(super) fn inside(base: &Path, rel: &Path) -> bool {
    let mut p = base.to_path_buf();
    for c in rel.components() {
        p.push(c);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(_) => return true,
        }
    }
    matches!((p.canonicalize(), base.canonicalize()), (Ok(a), Ok(b)) if a.starts_with(&b))
}

/// A file with more than one name: editing it edits them all.
#[cfg(unix)]
pub(super) fn other_names(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    m.nlink() > 1
}

#[cfg(not(unix))]
pub(super) fn other_names(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests;
