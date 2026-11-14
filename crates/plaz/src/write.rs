//! The write stage: the vault's code page ids in, the map's pages out.

use crate::render::{inline_code, link_text, Page};
use crate::Error;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The wiki's page id of each code path the vault lists.
pub struct Links {
    ids: BTreeMap<String, String>,
}

impl Links {
    /// Ids by path from `<vault>/index.json`. No file: none, silently. A file
    /// that is not a wiki index: none, and one warning.
    pub fn load(vault: &Path) -> Links {
        let path = vault.join("index.json");
        let ids = match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            read => read.ok().and_then(|b| index_ids(&b)).unwrap_or_else(|| {
                eprintln!(
                    "warning: {}: not a wiki index, code references will be plain",
                    path.display()
                );
                BTreeMap::new()
            }),
        };
        Links { ids }.existing(vault)
    }

    /// Only ids that cannot break a link and whose page `<vault>/<id>.md`
    /// (where the wiki writes it) exists.
    fn existing(mut self, vault: &Path) -> Links {
        self.ids.retain(|_, id| {
            !id.contains(['|', '[', ']', '#', '\n', '\r'])
                && vault.join(format!("{id}.md")).is_file()
        });
        self
    }

    /// `path:start-end` as a link to the wiki's page of `path` when the
    /// vault lists it, else as code.
    pub fn code(&self, path: &str, start: usize, end: usize) -> String {
        let span = format!("{path}:{start}-{end}");
        match self.ids.get(path) {
            Some(id) => format!("[[{id}|{}]]", link_text(&span)),
            None => inline_code(&span),
        }
    }
}

/// `{"entries": [{"id": …, "path": …}, …]}`, every entry with both strings.
fn index_ids(bytes: &[u8]) -> Option<BTreeMap<String, String>> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    v.get("entries")?
        .as_array()?
        .iter()
        .map(|e| {
            let s = |k: &str| e.get(k)?.as_str().map(str::to_string);
            Some((s("path")?, s("id")?))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub written: usize,
    pub unchanged: usize,
    pub removed: usize,
    /// Files at a page path that are not the map's, or not readable.
    pub left_alone: usize,
}

/// The temporary file a page is written to before it is renamed over its
/// path. The prune removes any such file a dead run left behind.
const TMP: &str = ".plaz-tmp";

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Writes `pages` under `<vault>/map/`, each through a temporary file renamed
/// over its path, leaving a file whose bytes are equal untouched and an empty
/// file taken for absent; then removes each `.md` file under `map/` that is
/// not in `pages` and whose front matter marks a map page (see
/// `is_map_page`), each temporary file of a dead run, and each directory
/// under `map/` left empty. Nothing else is removed.
pub fn write(vault: &Path, pages: &[Page]) -> Result<Written, Error> {
    let mut n = Written::default();
    let map = vault.join("map");
    std::fs::create_dir_all(&map).map_err(io(&map))?;
    let mut ours = BTreeSet::new();
    for page in pages {
        let path = vault.join(&page.rel_path);
        assert!(
            path.starts_with(&map),
            "a page outside map/: {}",
            page.rel_path
        );
        // Only a file that is not there is absent; one we cannot read may
        // be anyone's.
        let old = match std::fs::read(&path) {
            Ok(b) if b.is_empty() => None,
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                eprintln!("warning: {}: not readable, left alone", path.display());
                n.left_alone += 1;
                continue;
            }
        };
        if old.as_deref() == Some(page.content.as_bytes()) {
            n.unchanged += 1;
        } else if old.as_deref().is_some_and(|b| !is_map_page(b)) {
            eprintln!("warning: {}: not a map page, left alone", path.display());
            n.left_alone += 1;
        } else {
            let dir = path.parent().expect("under map/");
            std::fs::create_dir_all(dir).map_err(io(dir))?;
            let name = path.file_name().expect("a page file").to_string_lossy();
            let tmp = dir.join(format!(".{name}{TMP}"));
            // A leftover at the temporary path may be a link to anywhere:
            // remove it, never write through it.
            match std::fs::remove_file(&tmp) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(&tmp)(e)),
                Ok(()) | Err(_) => {}
            }
            std::fs::write(&tmp, &page.content).map_err(io(&tmp))?;
            std::fs::rename(&tmp, &path).map_err(io(&path))?;
            n.written += 1;
        }
        ours.insert(path);
    }
    n.removed = prune(&map, &ours)?;
    Ok(n)
}

/// Removes the stale map pages and temporary files under `dir`, then
/// `dir`'s empty subdirectories; the count of pages removed. A directory
/// that cannot be listed is left alone, with a warning.
fn prune(dir: &Path, ours: &BTreeSet<PathBuf>) -> Result<usize, Error> {
    let mut removed = 0;
    let listed = std::fs::read_dir(dir).and_then(|d| d.map(|e| e.map(|e| e.path())).collect());
    let Ok(mut entries): Result<Vec<PathBuf>, _> = listed else {
        eprintln!("warning: {}: not readable, left alone", dir.display());
        return Ok(0);
    };
    entries.sort();
    for path in entries {
        let kind = std::fs::symlink_metadata(&path).map_err(io(&path))?;
        if kind.is_dir() {
            removed += prune(&path, ours)?;
            if std::fs::read_dir(&path).is_ok_and(|mut d| d.next().is_none()) {
                std::fs::remove_dir(&path).map_err(io(&path))?;
            }
        } else if kind.is_file()
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(TMP))
        {
            std::fs::remove_file(&path).map_err(io(&path))?;
        } else if kind.is_file()
            && path.extension().is_some_and(|e| e == "md")
            && !ours.contains(&path)
            && std::fs::read(&path).is_ok_and(|b| is_map_page(&b))
        {
            std::fs::remove_file(&path).map_err(io(&path))?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Do `bytes` open with front matter holding both lines a map page writes,
/// `tags: ["plaz"]` and `kind: map`?
fn is_map_page(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return false;
    }
    let front: Vec<&str> = lines.take_while(|l| *l != "---").collect();
    front.contains(&"kind: map") && front.contains(&"tags: [\"plaz\"]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_front_matter_marks_a_map_page() {
        assert!(is_map_page(
            b"---\ntitle: x\ntags: [\"plaz\"]\nkind: map\n---\nbody\n"
        ));
        assert!(!is_map_page(b"---\ntitle: x\nkind: map\n---\nbody\n"));
        assert!(!is_map_page(
            b"---\ntitle: x\ntags: [\"logic-map\"]\nkind: map\n---\nbody\n"
        ));
        assert!(!is_map_page(b"---\ntitle: x\n---\nkind: map\n"));
        assert!(!is_map_page(b"kind: map\n"));
        assert!(!is_map_page(b"---\nkind: mapping\n---\n"));
    }

    #[test]
    fn code_links_only_listed_paths() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("index.json"),
            r#"{"entries":[{"id":"a","path":"src/a.rs","title":"A"}]}"#,
        )
        .unwrap();
        std::fs::write(d.path().join("a.md"), "a\n").unwrap();
        let links = Links::load(d.path());
        assert_eq!(links.code("src/a.rs", 1, 2), "[[a|src/a.rs:1-2]]");
        assert_eq!(links.code("src/b.rs", 3, 4), "`src/b.rs:3-4`");
        assert!(Links::load(&d.path().join("none")).ids.is_empty());
        assert_eq!(index_ids(br#"{"entries":[{"id":1,"path":"x"}]}"#), None);
        assert_eq!(index_ids(b"[]"), None);
    }
}
