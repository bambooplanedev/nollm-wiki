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
const BARRIER_PREFIX: &str = "barrier-";

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
            .join(format!("{BARRIER_PREFIX}{token}"));
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
                for token in ev.paths.iter().filter_map(|p| self.barrier_token(p)) {
                    s.barrier_seen = s.barrier_seen.max(token);
                }
                ev.need_rescan()
                    || ev.paths.iter().any(|p| {
                        !is_under(p, &self.dir)
                            && s.watch_set.as_ref().is_none_or(|w| w.relevant(p))
                    })
            }
            // A lost event may be a real change: count it.
            Err(_) => true,
        };
        if relevant {
            s.event_gen += 1;
        }
        self.changed.notify_all();
    }

    fn barrier_token(&self, path: &Path) -> Option<u64> {
        if path.parent()? != self.dir.join(BARRIER_DIR) {
            return None;
        }
        path.file_name()?
            .to_str()?
            .strip_prefix(BARRIER_PREFIX)?
            .parse()
            .ok()
    }

    /// Compile once and publish the result: a new snapshot on success, the
    /// error as the stale reason on failure (the old snapshot stays).
    fn compile_step(&self) -> Result<(), WikiError> {
        let target = self.lock().event_gen;
        let result = compile(&self.source, &self.dir, &self.opts)
            .and_then(|r| Ok((Wiki::load(&self.dir)?, r.watch_set)));
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
