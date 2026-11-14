//! Advisory file locks under `<dir>/.serve/`: a load never reads a wiki that a
//! compile is half way through writing.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) const SERVE_DIR: &str = ".serve";

/// `<dir>/.serve/<name>`, created if `.serve` exists; `.serve` itself is
/// never created here, so a dir no server ever used gets none.
fn open(dir: &Path, name: &str) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(SERVE_DIR).join(name))
}

/// Run `f` holding the exclusive `compile.lock`. Without a lock file (no
/// `.serve`, an odd filesystem) it runs `f` unlocked.
pub fn exclusive<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let lock = open(dir, "compile.lock").ok().filter(|l| l.lock().is_ok());
    let out = f();
    drop(lock);
    out
}

/// Run `f` holding the shared `compile.lock`; unlocked if it cannot be opened
/// (no `.serve`, a read-only `dir`).
pub fn shared<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let lock = open(dir, "compile.lock")
        .ok()
        .filter(|l| l.lock_shared().is_ok());
    let out = f();
    drop(lock);
    out
}

/// `shared` without waiting: `None` while a compile holds the lock.
pub fn try_shared<T>(dir: &Path, f: impl FnOnce() -> T) -> Option<T> {
    shared_until(dir, Instant::now(), f)
}

/// `shared` waiting no later than `until`: tries at least once, then every
/// 10 ms; `None` if a compile still holds the lock.
pub fn shared_until<T>(dir: &Path, until: Instant, f: impl FnOnce() -> T) -> Option<T> {
    let lock = open(dir, "compile.lock").ok();
    if let Some(l) = &lock {
        while let Err(std::fs::TryLockError::WouldBlock) = l.try_lock_shared() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            std::thread::sleep(left.min(Duration::from_millis(10)));
        }
    }
    let out = f();
    drop(lock);
    Some(out)
}

/// This server is the writer.
pub enum Writer {
    /// Holding `writer.lock` for as long as the file lives.
    Locked(File),
    /// `writer.lock` could not be opened or locked at all (permissions, NFS):
    /// a lone server must still compile.
    Unlocked,
}

/// `None` if another server holds the exclusive `writer.lock`. An error is
/// `Unlocked` only `at_start`: later, a transient one (EMFILE, NFS) must not
/// make a reader a second writer.
pub fn try_writer(dir: &Path, at_start: bool) -> Option<Writer> {
    let Ok(f) = open(dir, "writer.lock") else {
        return at_start.then_some(Writer::Unlocked);
    };
    match f.try_lock() {
        Ok(()) => Some(Writer::Locked(f)),
        Err(std::fs::TryLockError::WouldBlock) => None,
        Err(std::fs::TryLockError::Error(_)) => at_start.then_some(Writer::Unlocked),
    }
}

/// Test helper: a thread takes the exclusive lock on `dir`, runs `inside`,
/// then holds until the returned sender gets a message or `max` passes.
/// Returns once the lock is held and `inside` has run.
#[cfg(test)]
pub(crate) fn hold(
    dir: &Path,
    max: Duration,
    inside: impl FnOnce() + Send + 'static,
) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (held, held_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let dir = dir.to_path_buf();
    let thread = std::thread::spawn(move || {
        exclusive(&dir, || {
            inside();
            held.send(()).unwrap();
            let _ = release_rx.recv_timeout(max);
        });
    });
    held_rx.recv().unwrap();
    (release, thread)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_waits_for_exclusive() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().to_path_buf();
        std::fs::create_dir(d.join(SERVE_DIR)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let d2 = d.clone();
        let a = std::thread::spawn(move || {
            exclusive(&d2, || {
                tx.send(Instant::now()).unwrap();
                std::thread::sleep(Duration::from_millis(300));
            });
        });
        let start = rx.recv().unwrap();
        let at = shared(&d, Instant::now);
        a.join().unwrap();
        assert!(at.duration_since(start) >= Duration::from_millis(250));
    }

    #[cfg(unix)]
    #[test]
    fn shared_runs_without_lock_file_when_readonly() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let serve = t.path().join(SERVE_DIR);
        std::fs::create_dir(&serve).unwrap();
        std::fs::set_permissions(&serve, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root writes anyway: nothing to test.
        if std::fs::write(serve.join("probe"), "").is_err() {
            assert_eq!(shared(t.path(), || 7), 7);
            assert!(!serve.join("compile.lock").exists());
        }
        std::fs::set_permissions(&serve, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_missing_dir_is_not_created() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("typo");
        assert_eq!(exclusive(&d, || 1), 1);
        assert_eq!(shared(&d, || 2), 2);
        assert!(!d.exists());
    }

    #[test]
    fn exclusive_without_serve_dir_creates_nothing() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(exclusive(t.path(), || 3), 3);
        assert_eq!(shared(t.path(), || 4), 4);
        assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 0);
    }

    #[test]
    fn try_writer_without_a_lock_file_is_an_unlocked_writer() {
        let t = tempfile::tempdir().unwrap();
        // A directory in its place: opening it for writing fails, even as root.
        std::fs::create_dir_all(t.path().join(SERVE_DIR).join("writer.lock")).unwrap();
        assert!(matches!(try_writer(t.path(), true), Some(Writer::Unlocked)));
        assert!(try_writer(t.path(), false).is_none());
    }

    #[test]
    fn try_shared_does_not_wait_for_exclusive() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().to_path_buf();
        std::fs::create_dir(d.join(SERVE_DIR)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let d2 = d.clone();
        let a = std::thread::spawn(move || {
            exclusive(&d2, || {
                tx.send(()).unwrap();
                done_rx.recv().unwrap();
            });
        });
        rx.recv().unwrap();
        assert_eq!(try_shared(&d, || 1), None);
        done_tx.send(()).unwrap();
        a.join().unwrap();
        assert_eq!(try_shared(&d, || 2), Some(2));
    }

    #[test]
    fn shared_until_gives_up_at_the_deadline() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::create_dir(d.join(SERVE_DIR)).unwrap();
        let (release, a) = hold(d, Duration::from_secs(10), || ());
        let t = Instant::now();
        assert_eq!(shared_until(d, t + Duration::from_millis(200), || ()), None);
        assert!(
            t.elapsed() >= Duration::from_millis(200) && t.elapsed() < Duration::from_secs(1),
            "{:?}",
            t.elapsed()
        );
        release.send(()).unwrap();
        a.join().unwrap();
        assert_eq!(shared_until(d, Instant::now(), || 7), Some(7));
    }

    #[test]
    fn shared_until_runs_once_the_lock_frees() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::create_dir(d.join(SERVE_DIR)).unwrap();
        let (_release, a) = hold(d, Duration::from_millis(100), || ());
        let t = Instant::now();
        assert_eq!(shared_until(d, t + Duration::from_secs(2), || 1), Some(1));
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
        a.join().unwrap();
    }

    #[test]
    fn try_writer_is_exclusive() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(SERVE_DIR)).unwrap();
        let a = try_writer(t.path(), true);
        assert!(matches!(a, Some(Writer::Locked(_))));
        assert!(try_writer(t.path(), true).is_none());
        drop(a);
        assert!(matches!(
            try_writer(t.path(), false),
            Some(Writer::Locked(_))
        ));
    }
}
