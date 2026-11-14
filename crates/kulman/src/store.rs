//! `~/.kulman`: where it is, one run at a time, the state, and placing binaries.

use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::release::{self, Manifest};

/// The home directory, locked for this run.
pub struct Home {
    pub path: PathBuf,
    /// This run made the directory: a failed first install removes it again.
    pub created: bool,
    /// Holds the exclusive lock on `<home>/.lock` until dropped.
    _lock: File,
}

/// `KULMAN_HOME` (empty counts as unset; made absolute), else `$HOME/.kulman`.
pub fn home_path() -> Result<PathBuf> {
    let set = |key| env::var_os(key).filter(|v| !v.is_empty());
    let path = match (set("KULMAN_HOME"), set("HOME")) {
        (Some(home), _) => PathBuf::from(home),
        (None, Some(home)) => Path::new(&home).join(".kulman"),
        (None, None) => bail!("set HOME or KULMAN_HOME"),
    };
    Ok(std::path::absolute(path)?)
}

impl Home {
    /// Creates the directory if needed (its parent must exist), waits for the lock, then
    /// deletes stale temp entries. On failure, removes what it created.
    pub fn open(path: PathBuf) -> Result<Home> {
        let parent = path.parent().unwrap_or(Path::new("/"));
        if !parent.is_dir() {
            bail!("{} does not exist", parent.display());
        }
        let lock_path = path.join(".lock");
        // Twice at most: a failed first install or a `kulman remove` may remove home and its
        // lock file while this run waits for that lock.
        for _ in 0..2 {
            let created = match fs::create_dir(&path) {
                Ok(()) => true,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
                Err(e) => bail!("cannot create {}: {e}", path.display()),
            };
            let lock = match File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
            {
                // Home was removed since create_dir, by a run that held the lock: start again.
                // What is there now is not this run's to undo.
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                open => open
                    .and_then(|f| f.lock().map(|()| f))
                    .with_context(|| format!("cannot lock {}", lock_path.display())),
            };
            let lock = match lock {
                Ok(lock) => lock,
                Err(e) => {
                    undo_home(&path, created);
                    return Err(e);
                }
            };
            if !is_same_file(&lock, &lock_path) {
                continue;
            }
            let home = Home {
                path,
                created,
                _lock: lock,
            };
            if let Err(e) = home.clean_tmp() {
                home.abandon();
                return Err(e);
            }
            return Ok(home);
        }
        bail!(
            "cannot lock {}: it was removed while waiting",
            lock_path.display()
        )
    }

    /// Deletes every `<home>/.tmp-*`: under the lock, any found is left by a dead run.
    pub fn clean_tmp(&self) -> Result<()> {
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(".tmp-") {
                remove_any(&entry.path())?;
            }
        }
        Ok(())
    }

    /// This run's temp dir, `<home>/.tmp-<pid>`.
    pub fn tmp(&self) -> PathBuf {
        self.path.join(format!(".tmp-{}", std::process::id()))
    }

    /// After a failed run: removes the temp dir, and the home directory too if this run
    /// created it and nothing but the lock file is in it.
    pub fn abandon(self) {
        let _ = remove_any(&self.tmp());
        undo_home(&self.path, self.created);
    }
}

/// If this run created `path`: removes its lock file, then the directory if empty.
fn undo_home(path: &Path, created: bool) {
    if created {
        let _ = fs::remove_file(path.join(".lock"));
        let _ = fs::remove_dir(path);
    }
}

/// `file` is still the file at `path`.
fn is_same_file(file: &File, path: &Path) -> bool {
    match (file.metadata(), fs::metadata(path)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
}

fn remove_any(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// `<home>/state.json`: the installed version and modules. Written last: the commit record.
#[derive(Serialize, Deserialize)]
pub struct State {
    pub version: String,
    pub modules: Vec<String>,
}

impl State {
    /// The state in home directory `home`, read without the lock or creating anything.
    /// `None` when nothing is installed; malformed or unreadable is an error.
    pub fn load(home: &Path) -> Result<Option<State>> {
        let bytes = match fs::read(home.join("state.json")) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => bail!("cannot read state.json: {e}"),
        };
        let state: State = serde_json::from_slice(&bytes).context("state.json is malformed")?;
        if !release::is_version(&state.version) {
            bail!("state.json is malformed: bad version {:?}", state.version);
        }
        for name in &state.modules {
            release::check_name(name, "state.json")?;
        }
        Ok(Some(state))
    }
}

/// A binary unpacked, checked and ready in the temp dir.
pub struct Staged {
    pub name: String,
    pub path: PathBuf,
}

/// Fetches, checks and unpacks each of `names` (modules and `kulman`) into the temp dir;
/// nothing is fetched unless the release has an archive for every one. Nothing outside the
/// temp dir is touched.
pub fn stage(
    home: &Home,
    base: &str,
    manifest: &Manifest,
    target: &str,
    names: &[String],
) -> Result<Vec<Staged>> {
    for name in names {
        manifest.archive(name, target)?;
    }
    let tmp = home.tmp();
    fs::create_dir(&tmp)?;
    let mut staged = Vec::new();
    for name in names {
        let archive = manifest.archive(name, target)?;
        let file = tmp.join(format!("{name}.tar.gz"));
        release::fetch_to(base, &manifest.version, &archive.file, &file)
            .map_err(|e| anyhow::anyhow!("{name}: download failed: {e}"))?;
        if !release::sha256(&file)?.eq_ignore_ascii_case(&archive.sha256) {
            bail!("{name}: checksum mismatch");
        }
        let into = tmp.join(format!("{name}.unpack"));
        fs::create_dir(&into)?;
        let path = release::unpack_one(&file, name, &into)?;
        staged.push(Staged {
            name: name.clone(),
            path,
        });
    }
    Ok(staged)
}

/// Renames each staged binary (already synced) into `bin/` in the given order (the caller
/// puts kulman last), then writes `manifest.json`, then `state.json`, each through a synced
/// temp file. Removing the temp dir afterwards is best effort: the install is done.
pub fn commit(home: &Home, staged: &[Staged], manifest: &[u8], state: &State) -> Result<()> {
    let bin = home.path.join("bin");
    fs::create_dir_all(&bin)?;
    for s in staged {
        fs::rename(&s.path, bin.join(&s.name))?;
    }
    write(home, "manifest.json", manifest)?;
    write(home, "state.json", &serde_json::to_vec(state)?)?;
    let _ = fs::remove_dir_all(home.tmp());
    Ok(())
}

/// Replaces `<home>/<name>` by `bytes` through a synced file in the temp dir.
fn write(home: &Home, name: &str, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(home.tmp())?;
    let tmp = home.tmp().join(name);
    let mut file = File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, home.path.join(name))?;
    Ok(())
}

/// Deletes `bin/<name>` for each of `names`, then writes `state` (a crash in between leaves a
/// listed binary missing, which `kulman update` puts back).
pub fn remove_modules(home: &Home, names: &[String], state: &State) -> Result<()> {
    for name in names {
        delete(&home.path.join("bin").join(name))?;
    }
    write(home, "state.json", &serde_json::to_vec(state)?)?;
    let _ = fs::remove_dir_all(home.tmp());
    Ok(())
}

/// Deletes the binaries `state` lists and `bin/kulman`, then `manifest.json`, `.lock` and
/// `state.json` last; then `bin/` and home if empty. Nothing else is touched: any other file
/// keeps its directory.
pub fn remove_all(home: &Home, state: &State) -> Result<()> {
    let bin = home.path.join("bin");
    for name in state.modules.iter().map(String::as_str).chain(["kulman"]) {
        delete(&bin.join(name))?;
    }
    for name in ["manifest.json", ".lock", "state.json"] {
        delete(&home.path.join(name))?;
    }
    // remove_dir fails on a directory that is not empty, or is a symlink: both are kept.
    let _ = fs::remove_dir(&bin);
    let _ = fs::remove_dir(&home.path);
    Ok(())
}

/// Removes the file (or symlink) at `path`; one already missing counts as removed.
fn delete(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            bail!("cannot remove {}: {e}", path.display())
        }
        _ => Ok(()),
    }
}
