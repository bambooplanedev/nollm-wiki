//! `plaz setup`: the ignore line for the map's folder, and the map's removal.

use crate::{write, Error};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// `<source>/.gitignore`, shared with the team.
    Repo,
    /// The repository's `info/exclude`, for this clone only.
    Local,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The repository's `info/exclude`, `None` outside a repository or without git.
fn exclude_file(source: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    out.status.success().then(|| source.join(path.trim_end()))
}

fn has_line(path: &Path, line: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.lines().any(|l| l.trim() == line))
}

/// Makes sure `.kulman/` is ignored, in `.gitignore` or in the repository's
/// exclude file as `scope` says; a line in either file counts. Returns the
/// file it changed. Scope `Local` outside a repository says so on stdout.
pub fn ensure_ignored(source: &Path, scope: Scope) -> Result<Option<PathBuf>, Error> {
    let line = format!("{}/", wiki_code::OUT_DIR);
    let gitignore = source.join(".gitignore");
    let exclude = exclude_file(source);
    if has_line(&gitignore, &line) || exclude.as_deref().is_some_and(|e| has_line(e, &line)) {
        return Ok(None);
    }
    let target = match (scope, exclude) {
        (Scope::Repo, _) => gitignore,
        (Scope::Local, Some(exclude)) => exclude,
        (Scope::Local, None) => {
            println!("not a git repository: nothing to exclude");
            return Ok(None);
        }
    };
    let mut text = match std::fs::read_to_string(&target) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io(&target)(e)),
    };
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(io(dir))?;
    }
    std::fs::write(&target, text).map_err(io(&target))?;
    Ok(Some(target))
}

/// Removes the map's pages under `<vault>/map/` and the directories left
/// empty, `map/` included; the count removed. No `map/`: nothing is created.
pub fn remove(vault: &Path) -> Result<write::Written, Error> {
    let map = vault.join("map");
    if !map.is_dir() {
        return Ok(write::Written::default());
    }
    let n = write::write(vault, &[])?;
    if std::fs::read_dir(&map).is_ok_and(|mut d| d.next().is_none()) {
        std::fs::remove_dir(&map).map_err(io(&map))?;
    }
    Ok(n)
}
