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

pub fn upsert_entry(path: &Path, parents: &[&str], key: &str, value: &Value) -> Result<Outcome> {
    edit(path, parents, true, |m| match m.get(key) {
        Some(v) if v == value => Outcome::AlreadyThere,
        Some(_) => Outcome::Foreign(format!("\"{key}\" has another value")),
        None => {
            m.insert(key.to_string(), value.clone());
            Outcome::Done
        }
    })
}

/// Removes the directories under `root` that `path`'s removal left empty, deepest first.
fn prune_dirs(root: &Path, path: &Path) {
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

/// Deletes the file too (and the directories it leaves empty, up to `root`) when the entry was its last content.
pub fn remove_entry(
    root: &Path,
    path: &Path,
    parents: &[&str],
    key: &str,
    value: &Value,
) -> Result<Outcome> {
    let out = edit(path, parents, false, |m| match m.get(key) {
        None => Outcome::AlreadyThere,
        Some(v) if v == value => {
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

pub fn append_item(path: &Path, parents: &[&str], item: &Value) -> Result<Outcome> {
    item_edit(path, parents, true, |items| {
        if items.contains(item) {
            Outcome::AlreadyThere
        } else {
            items.push(item.clone());
            Outcome::Done
        }
    })
}

/// Like [`remove_entry`], for an array item.
pub fn remove_item(root: &Path, path: &Path, parents: &[&str], item: &Value) -> Result<Outcome> {
    let out = item_edit(path, parents, false, |items| {
        match items.iter().position(|v| v == item) {
            Some(i) => {
                items.remove(i);
                Outcome::Done
            }
            None => Outcome::AlreadyThere,
        }
    })?;
    if out == Outcome::Done && !path.exists() {
        prune_dirs(root, path);
    }
    Ok(out)
}

/// Writes `content`. A file holding any of `shipped` (an earlier release's text) is upgraded in
/// place; any other content is foreign.
pub fn write_file(path: &Path, content: &str, shipped: &[&str]) -> Result<Outcome> {
    match read_opt(path)? {
        Some(s) if s == content => Ok(Outcome::AlreadyThere),
        Some(s) if shipped.contains(&s.as_str()) => {
            std::fs::write(path, content)?;
            Ok(Outcome::Done)
        }
        Some(_) => Ok(foreign("file has other content")),
        None => {
            create_parent(path)?;
            std::fs::write(path, content)?;
            Ok(Outcome::Done)
        }
    }
}

/// Removes `path` when it holds exactly `content` or one of `shipped`, then the parent directories it leaves empty, up to but excluding `root`.
pub fn remove_file(root: &Path, path: &Path, content: &str, shipped: &[&str]) -> Result<Outcome> {
    match read_opt(path)? {
        None => return Ok(Outcome::AlreadyThere),
        Some(s) if s != content && !shipped.contains(&s.as_str()) => {
            return Ok(foreign("file has other content"))
        }
        Some(_) => {}
    }
    std::fs::remove_file(path)?;
    prune_dirs(root, path);
    Ok(Outcome::Done)
}

/// Adds `block` (which starts with the line `begin`) to a text file, after a blank line when the
/// file has other content. An earlier release's block (one of `shipped`) is replaced in place; a
/// file holding any other block that starts with `begin` is foreign.
pub fn upsert_block(path: &Path, begin: &str, block: &str, shipped: &[&str]) -> Result<Outcome> {
    let mut text = read_opt(path)?.unwrap_or_default();
    if text.contains(block) {
        return Ok(Outcome::AlreadyThere);
    }
    if let Some(old) = shipped.iter().find(|s| text.contains(**s)) {
        std::fs::write(path, text.replacen(old, block, 1))?;
        return Ok(Outcome::Done);
    }
    if text.lines().any(|l| l == begin) {
        return Ok(foreign("holds another wiki block"));
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

/// Takes out `block` (or an earlier release's, one of `shipped`) and the blank line
/// [`upsert_block`] put before it; deletes the file (and the directories it leaves empty) when
/// nothing else is left. Another block under the same begin line is foreign.
pub fn remove_block(root: &Path, path: &Path, block: &str, shipped: &[&str]) -> Result<Outcome> {
    let Some(text) = read_opt(path)? else {
        return Ok(Outcome::AlreadyThere);
    };
    let Some(block) = std::iter::once(block)
        .chain(shipped.iter().copied())
        .find(|b| text.contains(b))
    else {
        let begin = block.lines().next().unwrap_or_default();
        return Ok(if text.lines().any(|l| l == begin) {
            foreign("holds another wiki block")
        } else {
            Outcome::AlreadyThere
        });
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn pretty(v: &Value) -> String {
        serde_json::to_string_pretty(v).unwrap() + "\n"
    }

    fn is_foreign(o: &Outcome) -> bool {
        matches!(o, Outcome::Foreign(_))
    }

    #[test]
    fn upsert_creates_file_and_parents() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".mcp.json");
        let v = json!({"command": "wiki", "args": ["serve"]});
        assert_eq!(
            upsert_entry(&p, &["mcpServers"], "wiki", &v).unwrap(),
            Outcome::Done
        );
        let want = pretty(&json!({"mcpServers": {"wiki": v}}));
        assert_eq!(fs::read_to_string(&p).unwrap(), want);
        assert_eq!(
            upsert_entry(&p, &["mcpServers"], "wiki", &v).unwrap(),
            Outcome::AlreadyThere
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), want);
    }

    #[test]
    fn upsert_keeps_other_keys_and_order() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.json");
        let orig = r#"{"zeta":1,"mcpServers":{"other":{"command":"x"}},"alpha":2}"#;
        fs::write(&p, orig).unwrap();
        upsert_entry(&p, &["mcpServers"], "wiki", &json!({"command": "wiki"})).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&v), ["zeta", "mcpServers", "alpha"]);
        assert_eq!(keys(&v["mcpServers"]), ["other", "wiki"]);
    }

    #[test]
    fn upsert_reports_foreign_value() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.json");
        let orig = pretty(&json!({"mcpServers": {"wiki": {"command": "other"}}}));
        fs::write(&p, &orig).unwrap();
        let o = upsert_entry(&p, &["mcpServers"], "wiki", &json!({"command": "wiki"})).unwrap();
        assert!(is_foreign(&o));
        assert_eq!(fs::read_to_string(&p).unwrap(), orig);
    }

    #[test]
    fn upsert_refuses_malformed_json() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.json");
        fs::write(&p, "{ not json").unwrap();
        let o = upsert_entry(&p, &["mcpServers"], "wiki", &json!(1)).unwrap();
        assert!(is_foreign(&o));
        assert_eq!(fs::read_to_string(&p).unwrap(), "{ not json");
    }

    #[test]
    fn remove_deletes_a_file_left_empty() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".pi/a.json");
        let v = json!({"command": "wiki"});
        upsert_entry(&p, &["mcpServers"], "wiki", &v).unwrap();
        assert_eq!(
            remove_entry(d.path(), &p, &["mcpServers"], "wiki", &v).unwrap(),
            Outcome::Done
        );
        assert!(!p.exists());
        assert!(!d.path().join(".pi").exists());
        assert_eq!(
            remove_entry(d.path(), &p, &["mcpServers"], "wiki", &v).unwrap(),
            Outcome::AlreadyThere
        );
    }

    #[test]
    fn remove_restores_a_pretty_file_byte_for_byte() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.json");
        let orig = pretty(&json!({"b": 1, "mcpServers": {"other": {"command": "x"}}, "a": [1]}));
        fs::write(&p, &orig).unwrap();
        let v = json!({"command": "wiki"});
        upsert_entry(&p, &["mcpServers"], "wiki", &v).unwrap();
        remove_entry(d.path(), &p, &["mcpServers"], "wiki", &v).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), orig);
    }

    #[test]
    fn append_and_remove_item_keep_other_items() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        let (a, b) = (json!({"hooks": "A"}), json!({"hooks": "B"}));
        let path = ["hooks", "UserPromptSubmit"];
        fs::write(&p, pretty(&json!({"hooks": {"UserPromptSubmit": [a]}}))).unwrap();
        assert_eq!(append_item(&p, &path, &b).unwrap(), Outcome::Done);
        let read = || serde_json::from_str::<Value>(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(read()["hooks"]["UserPromptSubmit"], json!([a, b]));
        assert_eq!(append_item(&p, &path, &b).unwrap(), Outcome::AlreadyThere);
        assert_eq!(remove_item(d.path(), &p, &path, &b).unwrap(), Outcome::Done);
        assert_eq!(read()["hooks"]["UserPromptSubmit"], json!([a]));
        assert_eq!(remove_item(d.path(), &p, &path, &a).unwrap(), Outcome::Done);
        assert!(!p.exists());
    }

    #[test]
    fn write_and_remove_file_prune_empty_dirs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let p = root.join(".claude/skills/wiki-navigation/SKILL.md");
        fs::create_dir_all(root.join(".claude")).unwrap();
        fs::write(root.join(".claude/settings.json"), "{}").unwrap();
        assert_eq!(write_file(&p, "body", &[]).unwrap(), Outcome::Done);
        assert_eq!(write_file(&p, "body", &[]).unwrap(), Outcome::AlreadyThere);
        assert!(is_foreign(&write_file(&p, "other", &[]).unwrap()));
        assert!(is_foreign(&remove_file(root, &p, "other", &[]).unwrap()));
        assert_eq!(remove_file(root, &p, "body", &[]).unwrap(), Outcome::Done);
        assert!(!root.join(".claude/skills").exists());
        assert!(root.join(".claude").exists());
        assert_eq!(
            remove_file(root, &p, "body", &[]).unwrap(),
            Outcome::AlreadyThere
        );

        fs::remove_file(root.join(".claude/settings.json")).unwrap();
        write_file(&p, "body", &[]).unwrap();
        remove_file(root, &p, "body", &[]).unwrap();
        assert!(!root.join(".claude").exists());
        assert!(root.exists());
    }

    #[test]
    fn a_block_goes_after_other_text_and_comes_out_byte_for_byte() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("AGENTS.md");
        let (begin, block) = ("<!-- b -->", "<!-- b -->\nrules\n<!-- /b -->\n");
        fs::write(&p, "# Mine\n").unwrap();
        assert_eq!(upsert_block(&p, begin, block, &[]).unwrap(), Outcome::Done);
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            format!("# Mine\n\n{block}")
        );
        assert_eq!(
            upsert_block(&p, begin, block, &[]).unwrap(),
            Outcome::AlreadyThere
        );
        let other = "<!-- b -->\nold\n<!-- /b -->\n";
        assert!(is_foreign(&upsert_block(&p, begin, other, &[]).unwrap()));
        assert_eq!(
            remove_block(d.path(), &p, block, &[]).unwrap(),
            Outcome::Done
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), "# Mine\n");
        assert_eq!(
            remove_block(d.path(), &p, block, &[]).unwrap(),
            Outcome::AlreadyThere
        );

        let q = d.path().join("new/AGENTS.md");
        upsert_block(&q, begin, block, &[]).unwrap();
        assert_eq!(fs::read_to_string(&q).unwrap(), block);
        remove_block(d.path(), &q, block, &[]).unwrap();
        assert!(!d.path().join("new").exists());
    }

    #[test]
    fn shipped_texts_are_upgraded_and_removed_but_edits_are_foreign() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("AGENTS.md");
        let (begin, old, new) = (
            "<!-- b -->",
            "<!-- b -->\nold\n<!-- /b -->\n",
            "<!-- b -->\nnew\n<!-- /b -->\n",
        );
        fs::write(&p, format!("# Mine\n\n{old}")).unwrap();
        assert_eq!(upsert_block(&p, begin, new, &[old]).unwrap(), Outcome::Done);
        assert_eq!(fs::read_to_string(&p).unwrap(), format!("# Mine\n\n{new}"));
        fs::write(&p, format!("# Mine\n\n{old}")).unwrap();
        assert_eq!(
            remove_block(d.path(), &p, new, &[old]).unwrap(),
            Outcome::Done
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), "# Mine\n");
        fs::write(&p, "<!-- b -->\nedited\n<!-- /b -->\n").unwrap();
        assert!(is_foreign(&upsert_block(&p, begin, new, &[old]).unwrap()));
        assert!(is_foreign(
            &remove_block(d.path(), &p, new, &[old]).unwrap()
        ));
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "<!-- b -->\nedited\n<!-- /b -->\n"
        );

        let f = d.path().join("s/SKILL.md");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, "v1").unwrap();
        assert_eq!(write_file(&f, "v2", &["v1"]).unwrap(), Outcome::Done);
        assert_eq!(fs::read_to_string(&f).unwrap(), "v2");
        fs::write(&f, "mine").unwrap();
        assert!(is_foreign(&write_file(&f, "v2", &["v1"]).unwrap()));
        assert!(is_foreign(
            &remove_file(d.path(), &f, "v2", &["v1"]).unwrap()
        ));
        fs::write(&f, "v1").unwrap();
        assert_eq!(
            remove_file(d.path(), &f, "v2", &["v1"]).unwrap(),
            Outcome::Done
        );
    }

    #[test]
    fn ensure_line_appends_once() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".gitignore");
        fs::write(&p, "target/").unwrap();
        assert_eq!(ensure_line(&p, ".wiki/").unwrap(), Outcome::Done);
        assert_eq!(fs::read_to_string(&p).unwrap(), "target/\n.wiki/\n");
        assert_eq!(ensure_line(&p, ".wiki/").unwrap(), Outcome::AlreadyThere);
    }
}
