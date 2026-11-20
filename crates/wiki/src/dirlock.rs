//! Advisory file locks under `<dir>/.serve/`: a load never reads a wiki that a
//! compile is half way through writing.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const SERVE_DIR: &str = ".serve";

thread_local! {
    /// Set while this thread runs a call its client may cancel:
    /// [`shared_until`] stops waiting once it turns true.
    pub(crate) static CANCELLED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

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

/// `shared` waiting no later than `until`, or until this thread's call is
/// [`CANCELLED`]: tries at least once, then every 10 ms; `None` if a
/// compile still holds the lock.
pub fn shared_until<T>(dir: &Path, until: Instant, f: impl FnOnce() -> T) -> Option<T> {
    let lock = open(dir, "compile.lock").ok();
    if let Some(l) = &lock {
        while let Err(std::fs::TryLockError::WouldBlock) = l.try_lock_shared() {
            let left = until.saturating_duration_since(Instant::now());
            let cancelled =
                CANCELLED.with_borrow(|c| c.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)));
            if left.is_zero() || cancelled {
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
mod tests;
