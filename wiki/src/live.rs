//! `serve --source`: keeps a served wiki in step with its source tree.
//!
//! The notify callback counts relevant events into an event generation; a
//! background thread recompiles until the compiled generation catches up. A
//! tool call first creates a barrier file and waits for its event — events
//! arrive in order, so every edit made before the call has been counted by
//! then — and then waits for the compiled generation to reach the event
//! generation it saw. Both waits share one 5 s bound.

use crate::query::Wiki;
use crate::watch::{self, WatchSet};
use crate::{compile, is_under, CompileOptions, WikiError};
use notify::RecursiveMode;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long a tool call waits for a fresh snapshot before answering from the
/// one it has.
const WAIT: Duration = Duration::from_secs(5);

/// Barrier files go in this subdirectory of `--dir`: it stays writable when
/// the `--dir` root is not, and no `.md` name there is a page the prune knows.
const BARRIER_DIR: &str = ".serve";

struct State {
    /// Bumped by every relevant watch event.
    event_gen: u64,
    /// The `event_gen` the last finished compile started from.
    compiled_gen: u64,
    /// Last barrier token handed out, and the highest one the watcher saw.
    barrier_sent: u64,
    barrier_seen: u64,
    wiki: Arc<Wiki>,
    /// Why `wiki` may be behind the source: the last compile's error.
    stale: Option<String>,
    /// `None` until the first compile ends: then every event counts.
    watch_set: Option<WatchSet>,
}

pub(crate) struct Live {
    source: PathBuf,
    dir: PathBuf,
    opts: CompileOptions,
    state: Mutex<State>,
    changed: Condvar,
}

impl Live {
    /// Serve `dir` for `source`, recompiling on every relevant change. An
    /// existing wiki in `dir` is served at once while the start-up compile
    /// runs in the background; with none, the first compile runs here and its
    /// failure is the error.
    pub fn start(source: &Path, dir: &Path, opts: CompileOptions) -> Result<Arc<Live>, WikiError> {
        let named = |e: &dyn std::fmt::Display| {
            WikiError::Serve(format!("--source {}: {e}", source.display()))
        };
        let source = source.canonicalize().map_err(|e| named(&e))?;
        if !source.is_dir() {
            return Err(named(&"not a directory"));
        }
        let barriers = dir.join(BARRIER_DIR);
        std::fs::create_dir_all(&barriers)
            .map_err(|e| WikiError::Serve(format!("--dir {}: {e}", dir.display())))?;
        let dir = dir.canonicalize()?;
        let loaded = Wiki::load(&dir).ok();
        let missing = loaded.is_none();
        let live = Arc::new(Live {
            source,
            opts,
            state: Mutex::new(State {
                event_gen: 1,
                compiled_gen: 0,
                barrier_sent: 0,
                barrier_seen: 0,
                wiki: Arc::new(loaded.unwrap_or_default()),
                stale: None,
                watch_set: None,
            }),
            changed: Condvar::new(),
            dir,
        });

        let barriers = live.dir.join(BARRIER_DIR);
        let mut paths = vec![(live.source.as_path(), RecursiveMode::Recursive)];
        if !is_under(&barriers, &live.source) {
            paths.push((barriers.as_path(), RecursiveMode::NonRecursive));
        }
        // A memory folder outside the source is its own watch; one that does
        // not exist yet is left to the next start.
        if let Some(mem) = live.opts.memory.as_deref().filter(|m| m.is_dir()) {
            if !is_under(mem, &live.source) {
                paths.push((mem, RecursiveMode::Recursive));
            }
        }
        let hook = Arc::clone(&live);
        let watcher = watch::start(&paths, move |res| hook.on_event(&res))?;

        if missing {
            live.compile_step().map_err(|e| named(&e))?;
        }
        let worker = Arc::clone(&live);
        std::thread::spawn(move || {
            let _watcher = watcher;
            loop {
                drop(
                    worker
                        .changed
                        .wait_while(worker.lock(), |s| s.event_gen == s.compiled_gen)
                        .unwrap_or_else(PoisonError::into_inner),
                );
                std::thread::sleep(watch::DEBOUNCE);
                // A failure is kept in `stale`; the next edit retries.
                let _ = worker.compile_step();
            }
        });
        Ok(live)
    }

    /// The canonical output directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The snapshot holding every edit made before this call, and why it may
    /// not (the last compile failed, or the wait ran out). Blocks up to 5 s.
    pub fn fresh(&self) -> (Arc<Wiki>, Option<String>) {
        let deadline = Instant::now() + WAIT;
        let left = || deadline.saturating_duration_since(Instant::now());
        let mut s = self.lock();
        s.barrier_sent += 1;
        let token = s.barrier_sent;
        // Created under the lock, so tokens reach the watcher in token order;
        // one file per token, so an event names exactly the barrier it closes.
        let barrier = self
            .dir
            .join(BARRIER_DIR)
            .join(format!("{}{token}", barrier_prefix()));
        let wrote = std::fs::write(&barrier, "").is_ok();
        if wrote {
            s = self.wait(s, left(), |s| s.barrier_seen < token);
        }
        let target = s.event_gen;
        s = self.wait(s, left(), |s| s.compiled_gen < target);
        let timed_out = s.compiled_gen < target || (wrote && s.barrier_seen < token);
        let stale = if timed_out {
            Some("recompile still running".to_string())
        } else {
            s.stale.clone()
        };
        let wiki = Arc::clone(&s.wiki);
        drop(s);
        if wrote {
            let _ = std::fs::remove_file(&barrier);
        }
        (wiki, stale)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(
        &self,
        s: MutexGuard<'a, State>,
        timeout: Duration,
        pending: impl FnMut(&mut State) -> bool,
    ) -> MutexGuard<'a, State> {
        self.changed
            .wait_timeout_while(s, timeout, pending)
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }

    /// Runs on notify's thread, in event order.
    fn on_event(&self, res: &notify::Result<notify::Event>) {
        let mut s = self.lock();
        let relevant = match res {
            Ok(ev) => {
                for token in ev.paths.iter().filter_map(|p| barrier_token(&self.dir, p)) {
                    s.barrier_seen = s.barrier_seen.max(token);
                }
                is_edit(ev, &self.dir, s.watch_set.as_ref())
            }
            // A lost event may be a real change: count it.
            Err(_) => true,
        };
        if relevant {
            s.event_gen += 1;
        }
        self.changed.notify_all();
    }

    /// Compile once and publish the result: a new snapshot on success, the
    /// error as the stale reason on failure (the old snapshot stays).
    fn compile_step(&self) -> Result<(), WikiError> {
        let target = self.lock().event_gen;
        let result = unpanicked(|| {
            compile(&self.source, &self.dir, &self.opts)
                .and_then(|r| Ok((Wiki::load(&self.dir)?, r.watch_set)))
        });
        let mut s = self.lock();
        let outcome = match result {
            Ok((wiki, watch_set)) => {
                s.wiki = Arc::new(wiki);
                s.watch_set = Some(watch_set);
                s.stale = None;
                Ok(())
            }
            Err(e) => {
                s.stale = Some(e.to_string());
                Err(e)
            }
        };
        s.compiled_gen = target;
        self.changed.notify_all();
        outcome
    }
}

/// Does `ev` change what a compile into `dir` would see? `watch_set` is the
/// last compile's; with none yet, every path outside `dir` counts.
fn is_edit(ev: &notify::Event, dir: &Path, watch_set: Option<&WatchSet>) -> bool {
    ev.need_rescan()
        || watch::is_change(ev)
            && ev
                .paths
                .iter()
                .any(|p| !is_under(p, dir) && watch_set.is_none_or(|w| w.relevant(p)))
}

/// Barrier names are this prefix plus the token; the pid keeps two servers
/// sharing one `--dir` from closing each other's barriers.
fn barrier_prefix() -> String {
    format!("barrier-{}-", std::process::id())
}

/// The token of `path` if it is one of this process's barriers in `dir`.
fn barrier_token(dir: &Path, path: &Path) -> Option<u64> {
    if path.parent()? != dir.join(BARRIER_DIR) {
        return None;
    }
    path.file_name()?
        .to_str()?
        .strip_prefix(&barrier_prefix())?
        .parse()
        .ok()
}

/// `f`, with a panic turned into an error: the compile thread must outlive a
/// bug in one compile, or every later call would wait out its 5 s.
fn unpanicked<T>(f: impl FnOnce() -> Result<T, WikiError>) -> Result<T, WikiError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(std::io::Error::other("the compile panicked").into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, AccessMode, Flag, ModifyKind};
    use notify::{Event, EventKind};

    #[test]
    fn opening_a_walked_file_is_not_an_edit() {
        let dir = Path::new("/proj/.wiki");
        let file = PathBuf::from("/proj/src/a.rs");
        let set = WatchSet::new(
            Path::new("/proj"),
            [file.clone()],
            [PathBuf::from("/proj/src")],
        );
        let open =
            Event::new(EventKind::Access(AccessKind::Open(AccessMode::Any))).add_path(file.clone());
        let modify = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(file);
        let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert!(!is_edit(&open, dir, Some(&set)));
        assert!(!is_edit(&open, dir, None));
        assert!(is_edit(&modify, dir, Some(&set)));
        assert!(is_edit(&rescan, dir, Some(&set)));
    }

    #[test]
    fn barrier_token_reads_only_this_process_barriers() {
        let dir = Path::new("/out");
        let own = dir.join(BARRIER_DIR).join(format!("{}7", barrier_prefix()));
        let other = dir
            .join(BARRIER_DIR)
            .join(format!("barrier-{}-7", std::process::id() + 1));
        assert_eq!(barrier_token(dir, &own), Some(7));
        assert_eq!(barrier_token(dir, &other), None);
        assert_eq!(
            barrier_token(dir, &dir.join(format!("{}7", barrier_prefix()))),
            None
        );
    }

    #[test]
    fn a_panicking_compile_is_an_error() {
        let r: Result<(), WikiError> = unpanicked(|| panic!("boom"));
        assert_eq!(r.unwrap_err().to_string(), "the compile panicked");
    }
}
