//! `~/.kulman`: where it is, one run at a time, the state, and placing binaries.

use std::env;
use std::fs::{self, File};
use std::io;
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
    /// Creates the directory if needed, waits for the lock, then deletes stale temp entries.
    pub fn open(path: PathBuf) -> Result<Home> {
        let created = !path.exists();
        fs::create_dir_all(&path).with_context(|| format!("cannot create {}", path.display()))?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.join(".lock"))
            .and_then(|f| f.lock().map(|()| f))
            .with_context(|| format!("cannot lock {}", path.join(".lock").display()))?;
        let home = Home {
            path,
            created,
            _lock: lock,
        };
        home.clean_tmp()?;
        Ok(home)
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
        if self.created {
            let _ = fs::remove_file(self.path.join(".lock"));
            let _ = fs::remove_dir(&self.path);
        }
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
    /// `None` when nothing is installed; malformed or unreadable is an error.
    pub fn load(home: &Home) -> Result<Option<State>> {
        let bytes = match fs::read(home.path.join("state.json")) {
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

/// Fetches, checks and unpacks each of `names` (modules and `kulman`) into the temp dir.
/// Nothing outside the temp dir is touched.
pub fn stage(
    home: &Home,
    base: &str,
    manifest: &Manifest,
    target: &str,
    names: &[String],
) -> Result<Vec<Staged>> {
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

/// Renames each staged binary into `bin/` in the given order (the caller puts kulman last),
/// then writes `manifest.json`, then `state.json`, each through a temp file.
pub fn commit(home: &Home, staged: &[Staged], manifest: &[u8], state: &State) -> Result<()> {
    let bin = home.path.join("bin");
    fs::create_dir_all(&bin)?;
    for s in staged {
        fs::rename(&s.path, bin.join(&s.name))?;
    }
    let state = serde_json::to_vec(state)?;
    for (name, bytes) in [("manifest.json", manifest), ("state.json", &state)] {
        let tmp = home.tmp().join(name);
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, home.path.join(name))?;
    }
    fs::remove_dir_all(home.tmp())?;
    Ok(())
}
