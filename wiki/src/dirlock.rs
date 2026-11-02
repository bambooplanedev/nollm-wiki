//! Advisory file locks under `<dir>/.serve/`: a load never reads a wiki that a
//! compile is half way through writing.

use std::fs::{create_dir_all, File, OpenOptions};
use std::path::Path;

pub(crate) const SERVE_DIR: &str = ".serve";

fn open(dir: &Path, name: &str) -> std::io::Result<File> {
    let d = dir.join(SERVE_DIR);
    create_dir_all(&d)?;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(d.join(name))
}

/// Run `f` holding the exclusive `compile.lock`. Without a lock file (odd
/// filesystem) it runs `f` unlocked.
pub fn exclusive<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let lock = open(dir, "compile.lock").ok().filter(|l| l.lock().is_ok());
    let out = f();
    drop(lock);
    out
}

/// Run `f` holding the shared `compile.lock`; unlocked if it cannot be opened
/// (a read-only `dir`).
pub fn shared<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let lock = open(dir, "compile.lock")
        .ok()
        .filter(|l| l.lock_shared().is_ok());
    let out = f();
    drop(lock);
    out
}

/// The file holding the exclusive `writer.lock`, or `None` if another holder
/// has it. The lock lasts as long as the returned file.
pub fn try_writer(dir: &Path) -> Option<File> {
    let f = open(dir, "writer.lock").ok()?;
    f.try_lock().ok()?;
    Some(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn shared_waits_for_exclusive() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().to_path_buf();
        let start = Instant::now();
        let d2 = d.clone();
        let a = std::thread::spawn(move || {
            exclusive(&d2, || std::thread::sleep(Duration::from_millis(300)));
        });
        std::thread::sleep(Duration::from_millis(50));
        let at = shared(&d, Instant::now);
        a.join().unwrap();
        assert!(at.duration_since(start) >= Duration::from_millis(250));
    }

    #[cfg(unix)]
    #[test]
    fn shared_runs_without_lock_dir_when_readonly() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(shared(t.path(), || 7), 7);
        std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn try_writer_is_exclusive() {
        let t = tempfile::tempdir().unwrap();
        let a = try_writer(t.path());
        assert!(a.is_some());
        assert!(try_writer(t.path()).is_none());
        drop(a);
        assert!(try_writer(t.path()).is_some());
    }
}
