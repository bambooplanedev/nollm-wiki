//! `~/.kulman-home`: where it is, one run at a time, the state, and placing binaries.

use std::env;
use std::fs::{self, File, TryLockError};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::release::{self, Manifest};

/// The home directory, locked for this run.
pub struct Home {
    pub path: PathBuf,
    /// This run made the directory: a failed first install removes it again.
    pub created: bool,
    /// This run made `.lock`: a failed run with no state removes it again.
    created_lock: bool,
    /// Holds the exclusive lock on `<home>/.lock` until dropped.
    _lock: File,
}

/// `KULMAN_HOME` (empty counts as unset; made absolute), else `$HOME/.kulman-home`.
pub fn home_path() -> Result<PathBuf> {
    let set = |key| env::var_os(key).filter(|v| !v.is_empty());
    let path = match (set("KULMAN_HOME"), set("HOME")) {
        (Some(home), _) => PathBuf::from(home),
        (None, Some(home)) => Path::new(&home).join(".kulman-home"),
        (None, None) => bail!("set HOME or KULMAN_HOME"),
    };
    Ok(std::path::absolute(path)?)
}

impl Home {
    /// Creates the directory if needed (its parent must exist) and waits for the lock, saying
    /// so once when another run holds it. On failure, removes what it created. Stale temp
    /// entries are the caller's to clean.
    pub fn open(path: PathBuf) -> Result<Home> {
        let parent = path.parent().unwrap_or(Path::new("/"));
        if !parent.is_dir() {
            bail!("{} does not exist", parent.display());
        }
        let lock_path = path.join(".lock");
        let mut said = false;
        // Twice at most: a failed first install or a `kulman remove` may remove home and its
        // lock file while this run waits for that lock.
        for _ in 0..2 {
            let created = match fs::create_dir(&path) {
                Ok(()) => true,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
                Err(e) => bail!("cannot create {}: {e}", path.display()),
            };
            // The lock file, and whether this run made it.
            let open = |new| File::options().write(true).create_new(new).open(&lock_path);
            let opened = open(true).map(|f| (f, true)).or_else(|e| match e.kind() {
                io::ErrorKind::AlreadyExists => open(false).map(|f| (f, false)),
                _ => Err(e),
            });
            let lock = match opened {
                // Home was removed since create_dir, by a run that held the lock: start again.
                // What is there now is not this run's to undo.
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => Err(e),
                Ok((f, new)) => match f.try_lock() {
                    Ok(()) => Ok((f, new)),
                    Err(TryLockError::WouldBlock) => {
                        if !said {
                            let at = lock_path.display();
                            eprintln!("kulman: waiting for another kulman run ({at})");
                            said = true;
                        }
                        f.lock().map(|()| (f, new))
                    }
                    Err(TryLockError::Error(e)) => Err(e),
                },
            };
            let lock = lock.with_context(|| format!("cannot lock {}", lock_path.display()));
            let (lock, created_lock) = match lock {
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
                created_lock,
                _lock: lock,
            };
            if let Err(e) = home.check_bin() {
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

    /// `<home>/bin`, if present, is a real directory: never a symlink kulman would write or
    /// delete through. (Home itself may be a symlink.)
    fn check_bin(&self) -> Result<()> {
        let bin = self.path.join("bin");
        match fs::symlink_metadata(&bin) {
            Ok(m) if !m.is_dir() => bail!("{} is not a directory kulman can use", bin.display()),
            _ => Ok(()),
        }
    }

    /// Deletes every `<home>/.tmp-*`: under the lock, any found is left by a dead run. Only
    /// when a state records home as kulman's.
    pub fn clean_tmp(&self) -> Result<()> {
        let named = |p: &Path| p.display().to_string();
        for entry in fs::read_dir(&self.path).with_context(|| named(&self.path))? {
            let path = entry.with_context(|| named(&self.path))?.path();
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".tmp-"))
            {
                remove_any(&path).with_context(|| named(&path))?;
            }
        }
        Ok(())
    }

    /// This run's temp dir, `<home>/.tmp-<pid>`.
    pub fn tmp(&self) -> PathBuf {
        self.path.join(format!(".tmp-{}", std::process::id()))
    }

    /// After a failed run: removes the temp dir; with no state, the lock file if this run
    /// created it (still held, so a run waiting for it starts again); and the home directory if
    /// this run created it and it is now empty. A home found without state is left as found.
    pub fn abandon(self) {
        let _ = remove_any(&self.tmp());
        if self.created_lock && !self.path.join("state.json").try_exists().unwrap_or(true) {
            let _ = fs::remove_file(self.path.join(".lock"));
        }
        if self.created {
            let _ = fs::remove_dir(&self.path);
        }
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
    /// `None` when nothing is installed; malformed or unreadable is an error, each its own.
    pub fn load(home: &Path) -> Result<Option<State>> {
        let path = home.join("state.json");
        // Unreadable: the cause, never advice to delete a file that may be fine.
        let state = match fs::read(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => bail!("{}: {e}", path.display()),
            Ok(b) => serde_json::from_slice::<State>(&b).ok(),
        };
        let valid = |s: &State| {
            release::is_version(&s.version)
                && s.modules.iter().enumerate().all(|(i, name)| {
                    release::is_name(name) && name != "kulman" && !s.modules[..i].contains(name)
                })
        };
        match state {
            Some(state) if valid(&state) => Ok(Some(state)),
            _ => bail!(
                "{} is not a state file kulman wrote; fix it, or delete it and run kulman install",
                path.display()
            ),
        }
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
    fs::create_dir(&tmp).with_context(|| tmp.display().to_string())?;
    let mut staged = Vec::new();
    for name in names {
        let archive = manifest.archive(name, target)?;
        let file = tmp.join(format!("{name}.tar.gz"));
        release::fetch_to(base, &manifest.version, &archive.file, &file)
            .with_context(|| format!("{name}: download failed"))?;
        if !release::sha256(&file)?.eq_ignore_ascii_case(&archive.sha256) {
            bail!("{name}: checksum mismatch");
        }
        let into = tmp.join(format!("{name}.unpack"));
        fs::create_dir(&into).with_context(|| into.display().to_string())?;
        let path = release::unpack_one(&file, name, &into)?;
        staged.push(Staged {
            name: name.clone(),
            path,
        });
    }
    Ok(staged)
}

/// Set once `commit` starts changing files: a run that fails before that changed nothing.
pub static COMMITTING: AtomicBool = AtomicBool::new(false);

/// Renames each staged binary (already synced) into `bin/` in the given order (the caller
/// puts kulman last), then writes `manifest.json`, then `state.json`, each through a synced
/// temp file. Removing the temp dir afterwards is best effort: the install is done.
pub fn commit(home: &Home, staged: &[Staged], manifest: &[u8], state: &State) -> Result<()> {
    let bin = home.path.join("bin");
    let dests: Vec<PathBuf> = staged.iter().map(|s| bin.join(&s.name)).collect();
    let records = ["manifest.json", "state.json"].map(|n| home.path.join(n));
    check_files(&dests)?;
    check_files(&records)?;
    COMMITTING.store(true, Ordering::Relaxed);
    fs::create_dir_all(&bin).with_context(|| bin.display().to_string())?;
    for (s, dest) in staged.iter().zip(&dests) {
        fs::rename(&s.path, dest).with_context(|| dest.display().to_string())?;
    }
    write(home, "manifest.json", manifest)?;
    write(home, "state.json", &serde_json::to_vec(state)?)?;
    let _ = fs::remove_dir_all(home.tmp());
    Ok(())
}

/// Replaces `<home>/<name>` by `bytes`, ending with a newline, through a synced file in the
/// temp dir.
fn write(home: &Home, name: &str, bytes: &[u8]) -> Result<()> {
    let tmp = home.tmp().join(name);
    let dest = home.path.join(name);
    let end: &[u8] = if bytes.ends_with(b"\n") { b"" } else { b"\n" };
    fs::create_dir_all(home.tmp())
        .and_then(|()| File::create(&tmp))
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.write_all(end)?;
            file.sync_all()
        })
        .with_context(|| tmp.display().to_string())?;
    fs::rename(&tmp, &dest).with_context(|| dest.display().to_string())
}

/// With no state, kulman has no record of owning anything: deletes `.lock` only, then `bin/`
/// and home if empty. Returns whether home still holds anything.
pub fn remove_lock(home: &Home) -> Result<bool> {
    delete(&home.path.join(".lock"))?;
    let _ = fs::remove_dir(home.path.join("bin"));
    let _ = fs::remove_dir(&home.path);
    Ok(fs::read_dir(&home.path).is_ok_and(|mut d| d.next().is_some()))
}

/// Deletes `bin/<name>` for each of `names`, then writes `state` (a crash in between leaves a
/// listed binary missing, which `kulman update` puts back).
pub fn remove_modules(home: &Home, names: &[String], state: &State) -> Result<()> {
    let bin = home.path.join("bin");
    let paths: Vec<PathBuf> = names.iter().map(|n| bin.join(n)).collect();
    check_files(&paths)?;
    for path in &paths {
        delete(path)?;
    }
    write(home, "state.json", &serde_json::to_vec(state)?)?;
    let _ = fs::remove_dir_all(home.tmp());
    Ok(())
}

/// Deletes `bin/<module>` for each of `modules` and `bin/kulman`, then `manifest.json`,
/// `state.json` and `.lock` last (a run that arrives after the lock is gone finds no state);
/// then `bin/` and home if empty. Nothing else is touched: any other file keeps its directory.
pub fn remove_all(home: &Home, modules: &[String]) -> Result<()> {
    let bin = home.path.join("bin");
    let mut paths: Vec<PathBuf> = modules.iter().map(|n| bin.join(n)).collect();
    paths.push(bin.join("kulman"));
    check_files(&paths)?;
    // The records too, each a regular file or absent, before the first deletion.
    for name in ["manifest.json", "state.json"] {
        let path = home.path.join(name);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            _ => bail!("{} is not a file kulman wrote", path.display()),
        }
    }
    for path in &paths {
        delete(path)?;
    }
    for name in ["manifest.json", "state.json", ".lock"] {
        delete(&home.path.join(name))?;
    }
    // remove_dir fails on a directory that is not empty, or is a symlink: both are kept.
    let _ = fs::remove_dir(&bin);
    let _ = fs::remove_dir(&home.path);
    Ok(())
}

/// Each of `paths` is missing, a file or a symlink: checked before the first deletion.
fn check_files(paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        match fs::symlink_metadata(path) {
            Ok(m) if !m.is_file() && !m.is_symlink() => bail!("{} is not a file", path.display()),
            _ => {}
        }
    }
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
