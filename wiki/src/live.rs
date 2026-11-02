//! `serve --source`: keeps a served wiki in step with its source tree.
//!
//! The notify callback counts relevant events into an event generation; a
//! background thread recompiles until the compiled generation catches up. A
//! tool call first creates a barrier file and waits for its event — events
//! arrive in order, so every edit made before the call has been counted by
//! then — and then waits for the compiled generation to reach the event
//! generation it saw. Both waits share one 5 s bound, of which a call spends
//! at most 1 s on the start-up compile before answering from the old wiki.
//!
//! Several servers may share one `--dir`; only the one holding
//! `.serve/writer.lock` compiles, and it holds `.serve/compile.lock`
//! exclusively while it writes, so a load (under the shared lock) never reads
//! half a wiki. Every other server is a reader: its barrier is for the writer,
//! which notes the event generation it had on seeing it and, once a compile
//! has reached that, writes `ack-<id>-<token>` holding the compile's error or
//! nothing. The reader then reloads from disk. While it waits it keeps trying
//! for `writer.lock`, and on getting it finishes the call as the writer. The
//! OS drops the lock with its holder, so a dead writer leaves nothing behind.
//!
//! Once [`EXE`] sees the binary replaced, compiles stop for good: an old
//! compiler must not overwrite what the new version wrote. It gives up
//! `writer.lock` too, so a server that still compiles takes over.

use crate::query::Wiki;
use crate::serve::Fingerprint;
use crate::watch::{self, WatchSet};
use crate::{compile, is_under, CompileOptions, WikiError};
use notify::RecursiveMode;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

/// How long a tool call waits for a fresh snapshot before answering from the
/// one it has.
const WAIT: Duration = Duration::from_secs(5);

/// How much of `WAIT` a call gives the start-up compile: enough for a small
/// project, while a large one answers from the old wiki without the full wait.
const START_WAIT: Duration = Duration::from_secs(1);

/// The stale reason while a compile has yet to catch up.
const RUNNING: &str = "recompile still running";

/// Barrier files go in this subdirectory of `--dir`: it stays writable when
/// the `--dir` root is not, and no `.md` name there is a page the prune knows.
use crate::dirlock::SERVE_DIR as BARRIER_DIR;

/// What tells a file from its replacement. A stat, cheap enough for every
/// tool call.
#[derive(PartialEq)]
struct FileId {
    /// 0 where there are no inodes.
    inode: u64,
    mtime: SystemTime,
    len: u64,
}

fn file_id(path: &Path) -> std::io::Result<FileId> {
    let meta = std::fs::metadata(path)?;
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&meta);
    #[cfg(not(unix))]
    let inode = 0;
    Ok(FileId {
        inode,
        mtime: meta.modified()?,
        len: meta.len(),
    })
}

/// The executable this process started from, kept to see it replaced.
pub(crate) struct Exe {
    files: Vec<(PathBuf, FileId)>,
    replaced: AtomicBool,
}

/// This process's executable; `serve` forces it at start.
pub(crate) static EXE: LazyLock<Exe> = LazyLock::new(|| match std::env::current_exe() {
    Ok(exe) => Exe::of(std::iter::once(exe).chain(invoked_path())),
    Err(_) => Exe::of([]),
});

impl Exe {
    /// Track `paths`, the first being the running binary. A later one counts
    /// only if it is that same file, so an `argv[0]` naming something else
    /// never reads as a replacement. Nothing readable means no check.
    fn of(paths: impl IntoIterator<Item = PathBuf>) -> Exe {
        let mut files: Vec<(PathBuf, FileId)> = Vec::new();
        for path in paths {
            if let Ok(id) = file_id(&path) {
                if files.first().is_none_or(|(_, first)| *first == id) {
                    files.push((path, id));
                }
            }
        }
        Exe {
            files,
            replaced: AtomicBool::new(false),
        }
    }

    /// Has a tracked path changed or gone? Once true, always true; the first
    /// time it says so on stderr.
    pub(crate) fn replaced(&self) -> bool {
        if self.replaced.load(Ordering::Relaxed) {
            return true;
        }
        let same = |(path, id): &(PathBuf, FileId)| file_id(path).is_ok_and(|now| now == *id);
        if self.files.iter().all(same) {
            return false;
        }
        if !self.replaced.swap(true, Ordering::Relaxed) {
            eprintln!("wiki serve: {OUTDATED}");
        }
        true
    }
}

/// Why a replaced binary's answers may be stale, and what to do about it.
pub(crate) const OUTDATED: &str = "the wiki server is out of date, its binary was replaced or \
removed; restart the agent session or reconnect the MCP server";

/// The path this process was started as, which unlike `current_exe` on Linux
/// keeps a symlink: `argv[0]`, looked up on `PATH` when it is a bare name.
fn invoked_path() -> Option<PathBuf> {
    let arg0 = PathBuf::from(std::env::args_os().next()?);
    if arg0.components().count() > 1 {
        return std::path::absolute(arg0).ok();
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(&arg0))
        .find(|p| p.is_file())
}

struct State {
    /// Bumped by every relevant watch event.
    event_gen: u64,
    /// The `event_gen` the last finished compile started from; in a reader,
    /// 1 once it has loaded a wiki.
    compiled_gen: u64,
    /// A compile is running.
    compiling: bool,
    /// Last barrier token handed out, and the highest one the watcher saw.
    barrier_sent: u64,
    barrier_seen: u64,
    /// This server's barrier tokens the writer has acked, not yet answered.
    acked: Vec<u64>,
    /// Writer: other servers' barriers to ack, as `<id>-<token>` and the
    /// `event_gen` the compile must reach first.
    foreign: Vec<(String, u64)>,
    /// Writer: the highest barrier token seen per other server, so a late
    /// event for an answered barrier is not acked again.
    foreign_seen: HashMap<String, u64>,
    wiki: Arc<Wiki>,
    /// Reader: `index.json`'s fingerprint when `wiki` was loaded; an ack
    /// reloads only when it changed.
    on_disk: Option<Fingerprint>,
    /// Why `wiki` may be behind the source: the last compile's error.
    stale: Option<String>,
    /// `None` until the first compile ends: then every event counts.
    watch_set: Option<WatchSet>,
}

pub(crate) struct Live {
    source: PathBuf,
    dir: PathBuf,
    opts: CompileOptions,
    /// Names this server's barriers and acks: the pid and a per-process count.
    id: String,
    /// The held `writer.lock`, while this server is the writer.
    writer: Mutex<Option<File>>,
    state: Mutex<State>,
    changed: Condvar,
    #[cfg(test)]
    compiles: std::sync::atomic::AtomicUsize,
}

impl Live {
    /// Serve `dir` for `source`. The writer recompiles on every relevant
    /// change: an existing wiki in `dir` is served at once while its start-up
    /// compile runs in the background; with none, the first compile runs here
    /// and its failure is the error. A reader never compiles: it serves the
    /// wiki on disk, or an empty one until the writer's first ack.
    pub fn start(source: &Path, dir: &Path, opts: CompileOptions) -> Result<Arc<Live>, WikiError> {
        static COUNT: AtomicU64 = AtomicU64::new(0);
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
        let live = Arc::new(Live {
            source,
            opts,
            id: server_id(COUNT.fetch_add(1, Ordering::Relaxed)),
            writer: Mutex::new(None),
            state: Mutex::new(State {
                event_gen: 1,
                compiled_gen: 0,
                compiling: false,
                barrier_sent: 0,
                barrier_seen: 0,
                acked: Vec::new(),
                foreign: Vec::new(),
                foreign_seen: HashMap::new(),
                wiki: Arc::default(),
                on_disk: None,
                stale: None,
                watch_set: None,
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            compiles: std::sync::atomic::AtomicUsize::new(0),
            dir,
        });
        let writer = live.try_become_writer();
        let missing = live.reload().is_err();
        if !missing && !writer {
            live.lock().compiled_gen = 1;
        }

        let barriers = live.dir.join(BARRIER_DIR);
        let mut paths = vec![(live.source.as_path(), RecursiveMode::Recursive)];
        if !is_under(&barriers, &live.source) {
            paths.push((barriers.as_path(), RecursiveMode::NonRecursive));
        }
        // A memory folder outside the source is its own watch; one that does
        // not exist yet is watched through its parent, so creating it counts.
        for mem in live
            .opts
            .memory
            .iter()
            .filter(|m| !is_under(m, &live.source))
        {
            let target = if mem.is_dir() {
                Some(mem.as_path())
            } else {
                mem.parent().filter(|p| p.is_dir())
            };
            if let Some(t) = target {
                paths.push((t, RecursiveMode::Recursive));
            }
        }
        let hook = Arc::clone(&live);
        let watcher = watch::start(&paths, move |res| hook.on_event(&res))?;

        if missing && writer {
            live.compile_step().map_err(|e| named(&e))?;
        }
        let worker = Arc::clone(&live);
        std::thread::spawn(move || {
            let _watcher = watcher;
            loop {
                // A reader idles here until it takes over as the writer.
                drop(
                    worker
                        .changed
                        .wait_while(worker.lock(), |s| {
                            s.event_gen == s.compiled_gen || !worker.is_writer()
                        })
                        .unwrap_or_else(PoisonError::into_inner),
                );
                std::thread::sleep(watch::DEBOUNCE);
                if worker.is_writer() {
                    // A failure is kept in `stale`; the next edit retries.
                    let _ = worker.compile_step();
                }
            }
        });
        Ok(live)
    }

    /// The canonical output directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The snapshot compiled last, without waiting for anything.
    pub fn wiki(&self) -> Arc<Wiki> {
        Arc::clone(&self.lock().wiki)
    }

    fn is_writer(&self) -> bool {
        self.writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Take `writer.lock` if it is free. A new writer has edits to catch up
    /// with, and acks the barriers other servers left waiting.
    fn try_become_writer(&self) -> bool {
        if EXE.replaced() {
            return false;
        }
        {
            let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
            if writer.is_some() {
                return true;
            }
            *writer = crate::dirlock::try_writer(&self.dir);
            if writer.is_none() {
                return false;
            }
        }
        let mut s = self.lock();
        s.event_gen += 1;
        if let Ok(entries) = std::fs::read_dir(self.dir.join(BARRIER_DIR)) {
            // In token order: a later token noted first would hide an earlier one.
            let mut found: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter_map(|p| Some((self.foreign_barrier(&p)?, p)))
                .collect();
            found.sort();
            for (_, path) in found {
                self.note_barrier(&mut s, &path);
            }
        }
        self.changed.notify_all();
        true
    }

    #[cfg(test)]
    fn release_writer(&self) {
        *self.writer.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// The snapshot holding every edit made before this call, and why it may
    /// not (the last compile failed, or the wait ran out). Blocks up to 5 s,
    /// or 1 s while the start-up compile runs.
    pub fn fresh(&self) -> (Arc<Wiki>, Option<String>) {
        let deadline = Instant::now() + WAIT;
        if !self.is_writer() {
            if let Some(answer) = self.await_ack(deadline) {
                return answer;
            }
        }
        let left = || deadline.saturating_duration_since(Instant::now());
        let mut s = self.lock();
        if s.compiled_gen == 0 {
            s = self.wait(s, START_WAIT.min(left()), |s| s.compiled_gen == 0);
            if s.compiled_gen == 0 {
                return (Arc::clone(&s.wiki), Some(RUNNING.to_string()));
            }
        }
        let (token, barrier, wrote) = self.put_barrier(&mut s);
        if wrote {
            s = self.wait(s, left(), |s| s.barrier_seen < token);
        }
        let target = s.event_gen;
        s = self.wait(s, left(), |s| s.compiled_gen < target);
        let timed_out = s.compiled_gen < target || (wrote && s.barrier_seen < token);
        let stale = if timed_out {
            Some(RUNNING.to_string())
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

    /// A new barrier file. Created under the lock, so tokens reach the
    /// watcher in token order; one file per token, so an event names exactly
    /// the barrier it closes.
    fn put_barrier(&self, s: &mut State) -> (u64, PathBuf, bool) {
        s.barrier_sent += 1;
        let token = s.barrier_sent;
        let barrier = self
            .dir
            .join(BARRIER_DIR)
            .join(format!("barrier-{}-{token}", self.id));
        let wrote = std::fs::write(&barrier, "").is_ok();
        (token, barrier, wrote)
    }

    /// A reader's `fresh`: put a barrier, wait for the writer to ack it, and
    /// reload. `None` if this server took over as the writer meanwhile.
    fn await_ack(&self, deadline: Instant) -> Option<(Arc<Wiki>, Option<String>)> {
        let mut s = self.lock();
        let end = if s.compiled_gen == 0 {
            deadline.min(Instant::now() + START_WAIT)
        } else {
            deadline
        };
        let (token, barrier, _) = self.put_barrier(&mut s);
        let ack = self
            .dir
            .join(BARRIER_DIR)
            .join(format!("ack-{}-{token}", self.id));
        let acked = loop {
            let left = end.saturating_duration_since(Instant::now());
            s = self.wait(s, left.min(watch::DEBOUNCE), |s| !s.acked.contains(&token));
            if s.acked.contains(&token) {
                s.acked.retain(|&t| t != token);
                break true;
            }
            drop(s);
            if self.try_become_writer() {
                let _ = std::fs::remove_file(&barrier);
                let _ = std::fs::remove_file(&ack);
                return None;
            }
            s = self.lock();
            if Instant::now() >= end {
                break false;
            }
        };
        drop(s);
        let mut stale = Some(RUNNING.to_string());
        if acked {
            stale = match std::fs::read_to_string(&ack) {
                Ok(reason) => Some(reason).filter(|r| !r.is_empty()),
                Err(e) => Some(e.to_string()),
            };
            let unchanged = crate::serve::fingerprint(&self.dir)
                .is_ok_and(|now| Some(now) == self.lock().on_disk);
            match if unchanged { Ok(()) } else { self.reload() } {
                Ok(()) => {
                    let mut s = self.lock();
                    s.compiled_gen = s.compiled_gen.max(1);
                }
                Err(e) => stale = stale.or(Some(e)),
            }
        }
        let _ = std::fs::remove_file(&barrier);
        let _ = std::fs::remove_file(&ack);
        // An ack event that came in after the wait ended.
        let mut s = self.lock();
        s.acked.retain(|&t| t != token);
        Some((Arc::clone(&s.wiki), stale))
    }

    /// Load the wiki on disk under the shared lock, keeping its fingerprint.
    /// On an error the old snapshot stays.
    fn reload(&self) -> Result<(), String> {
        let (on_disk, loaded) = crate::dirlock::shared(&self.dir, || {
            (
                crate::serve::fingerprint(&self.dir).ok(),
                Wiki::load(&self.dir),
            )
        });
        let wiki = loaded.map_err(|e| e.to_string())?;
        let mut s = self.lock();
        s.wiki = Arc::new(wiki);
        s.on_disk = on_disk;
        Ok(())
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
                for path in &ev.paths {
                    if let Some(token) = own_token(&self.dir, "barrier", &self.id, path) {
                        s.barrier_seen = s.barrier_seen.max(token);
                    }
                    if let Some(token) = own_token(&self.dir, "ack", &self.id, path) {
                        // Only while its call waits: that call still has its barrier.
                        let barrier = path.with_file_name(format!("barrier-{}-{token}", self.id));
                        if path.exists() && barrier.exists() && !s.acked.contains(&token) {
                            s.acked.push(token);
                        }
                    }
                }
                if ev.paths.iter().any(|p| self.foreign_barrier(p).is_some()) && self.is_writer() {
                    if EXE.replaced() {
                        // Hand the dir to a server that still compiles.
                        *self.writer.lock().unwrap_or_else(PoisonError::into_inner) = None;
                    } else {
                        for path in &ev.paths {
                            self.note_barrier(&mut s, path);
                        }
                        if !s.compiling {
                            self.ack_ready(&mut s);
                        }
                    }
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

    /// `<id>-<token>` of `path` if it is another server's barrier in `dir`.
    fn foreign_barrier(&self, path: &Path) -> Option<(String, u64)> {
        if path.parent()? != self.dir.join(BARRIER_DIR) {
            return None;
        }
        let (id, token) = path
            .file_name()?
            .to_str()?
            .strip_prefix("barrier-")?
            .rsplit_once('-')?;
        let token = token.parse().ok()?;
        (id != self.id).then(|| (id.to_string(), token))
    }

    /// Writer: if `path` is another server's barrier not yet seen, owe it an
    /// ack once a compile reaches the current `event_gen`.
    fn note_barrier(&self, s: &mut State, path: &Path) {
        let Some((id, token)) = self.foreign_barrier(path) else {
            return;
        };
        if s.foreign_seen.get(&id).is_some_and(|&t| t >= token) || !path.exists() {
            return;
        }
        s.foreign.push((format!("{id}-{token}"), s.event_gen));
        s.foreign_seen.insert(id, token);
    }

    /// Writer: ack every barrier the last compile covers. The ack holds the
    /// stale reason, empty for none, and appears whole by a rename.
    fn ack_ready(&self, s: &mut State) {
        let done = s.compiled_gen;
        let serve = self.dir.join(BARRIER_DIR);
        let reason = s.stale.clone().unwrap_or_default();
        s.foreign.retain(|(name, gen)| {
            if *gen > done {
                return true;
            }
            // A reader that gave up has removed its barrier: no ack to leave behind.
            if serve.join(format!("barrier-{name}")).exists() {
                let tmp = serve.join(format!("tmp-ack-{name}"));
                if std::fs::write(&tmp, &reason).is_ok() {
                    let _ = std::fs::rename(&tmp, serve.join(format!("ack-{name}")));
                }
            }
            false
        });
    }

    /// Compile once and publish the result: a new snapshot on success, the
    /// error as the stale reason on failure (the old snapshot stays), and acks
    /// for the readers it covers. With the binary replaced it only marks the
    /// edits as handled and gives up the writer lock.
    fn compile_step(&self) -> Result<(), WikiError> {
        let target = {
            let mut s = self.lock();
            s.compiling = true;
            s.event_gen
        };
        if EXE.replaced() {
            *self.writer.lock().unwrap_or_else(PoisonError::into_inner) = None;
            let mut s = self.lock();
            s.compiling = false;
            s.compiled_gen = target;
            self.changed.notify_all();
            return Ok(());
        }
        #[cfg(test)]
        self.compiles.fetch_add(1, Ordering::Relaxed);
        let result = unpanicked(|| {
            crate::dirlock::exclusive(&self.dir, || {
                compile(&self.source, &self.dir, &self.opts)
                    .and_then(|r| Ok((Wiki::load(&self.dir)?, r.watch_set)))
            })
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
        s.compiling = false;
        s.compiled_gen = target;
        self.ack_ready(&mut s);
        self.changed.notify_all();
        outcome
    }
}

/// Names this server's barriers and acks; `n` counts servers in this process.
/// The start time keeps a later process that reuses a pid from inheriting
/// the token high-water mark the writer keeps for the old one.
fn server_id(n: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}.{nanos}.{n}", std::process::id())
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

/// The token of `path` if it is the server `id`'s `kind` file (`barrier` or
/// `ack`) in `dir`: `<kind>-<id>-<token>`. The id keeps servers sharing one
/// `--dir` from closing each other's barriers.
fn own_token(dir: &Path, kind: &str, id: &str, path: &Path) -> Option<u64> {
    if path.parent()? != dir.join(BARRIER_DIR) {
        return None;
    }
    path.file_name()?
        .to_str()?
        .strip_prefix(&format!("{kind}-{id}-"))?
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
    fn own_token_reads_only_this_server_files() {
        let dir = Path::new("/out");
        let serve = dir.join(BARRIER_DIR);
        assert_eq!(
            own_token(dir, "barrier", "9.0", &serve.join("barrier-9.0-7")),
            Some(7)
        );
        assert_eq!(
            own_token(dir, "ack", "9.0", &serve.join("ack-9.0-7")),
            Some(7)
        );
        assert_eq!(
            own_token(dir, "ack", "9.0", &serve.join("barrier-9.0-7")),
            None
        );
        assert_eq!(
            own_token(dir, "barrier", "9.0", &serve.join("barrier-9.1-7")),
            None
        );
        assert_eq!(
            own_token(dir, "ack", "9.0", &serve.join("tmp-ack-9.0-7")),
            None
        );
        assert_eq!(
            own_token(dir, "barrier", "9.0", &dir.join("barrier-9.0-7")),
            None
        );
    }

    #[test]
    fn a_transcript_beside_a_missing_memory_folder_does_not_recompile() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.md"), "# A\n\nalpha\n").unwrap();
        let out = tempfile::tempdir().unwrap();
        let slot = tempfile::tempdir().unwrap();
        let slot = slot.path().canonicalize().unwrap();
        let opts = CompileOptions {
            incremental: true,
            memory: vec![slot.join("memory")],
            ..CompileOptions::default()
        };
        let live = Live::start(src.path(), out.path(), opts).unwrap();
        let _ = live.fresh();
        let gens = |l: &Live| {
            let s = l.lock();
            (s.event_gen, s.compiled_gen)
        };
        let before = gens(&live);

        std::fs::write(slot.join("abc.jsonl"), "{}\n").unwrap();
        std::thread::sleep(watch::DEBOUNCE * 4);
        assert_eq!(gens(&live), before);

        std::fs::create_dir(slot.join("memory")).unwrap();
        std::fs::write(slot.join("memory/n.md"), "# N\n\nnote\n").unwrap();
        let _ = live.fresh();
        assert!(gens(&live).1 > before.1);
    }

    /// `<dir>/wiki` with `body`, and an `Exe` tracking it.
    fn tracked(dir: &Path, body: &str) -> (PathBuf, Exe) {
        let bin = dir.join("wiki");
        std::fs::write(&bin, body).unwrap();
        let exe = Exe::of([bin.clone()]);
        (bin, exe)
    }

    #[test]
    fn an_untouched_binary_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (_bin, exe) = tracked(dir.path(), "one");
        assert!(!exe.replaced());
        assert!(!exe.replaced());
    }

    #[test]
    fn a_rename_over_the_binary_stays_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, exe) = tracked(dir.path(), "one");
        let saved = dir.path().join("saved");
        std::fs::copy(&bin, &saved).unwrap();
        let new = dir.path().join("new");
        std::fs::write(&new, "one").unwrap();
        std::fs::rename(&new, &bin).unwrap();
        assert!(exe.replaced());
        std::fs::rename(&saved, &bin).unwrap();
        assert!(exe.replaced());
    }

    #[test]
    fn a_rebuild_in_place_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, exe) = tracked(dir.path(), "one");
        std::fs::write(&bin, "a longer one").unwrap();
        assert!(exe.replaced());
    }

    #[test]
    fn a_removed_binary_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, exe) = tracked(dir.path(), "one");
        std::fs::remove_file(&bin).unwrap();
        assert!(exe.replaced());
    }

    #[test]
    fn no_readable_executable_means_no_check() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!Exe::of([dir.path().join("gone")]).replaced());
        assert!(!Exe::of([]).replaced());
    }

    #[test]
    fn a_path_that_is_another_file_is_not_tracked() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, _) = tracked(dir.path(), "one");
        let other = dir.path().join("other");
        std::fs::write(&other, "something else").unwrap();
        let exe = Exe::of([bin, other.clone()]);
        std::fs::remove_file(&other).unwrap();
        assert!(!exe.replaced());
    }

    #[cfg(unix)]
    #[test]
    fn a_retargeted_symlink_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, _) = tracked(dir.path(), "one");
        let other = dir.path().join("other");
        std::fs::write(&other, "one").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&bin, &link).unwrap();
        let exe = Exe::of([bin, link.clone()]);
        assert!(!exe.replaced());
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(exe.replaced());
    }

    /// Two servers on one source and one `--dir`, the first the writer.
    fn pair() -> (tempfile::TempDir, tempfile::TempDir, Arc<Live>, Arc<Live>) {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.md"), "# A\n\nalpha\n").unwrap();
        let out = tempfile::tempdir().unwrap();
        let a = Live::start(src.path(), out.path(), CompileOptions::default()).unwrap();
        let b = Live::start(src.path(), out.path(), CompileOptions::default()).unwrap();
        (src, out, a, b)
    }

    fn finds(wiki: &Wiki, word: &str) -> bool {
        !wiki.search(word, None, 10).is_empty()
    }

    #[test]
    fn second_live_on_one_dir_is_a_reader() {
        let (_src, _out, a, b) = pair();
        assert!(a.is_writer());
        assert!(!b.is_writer());
    }

    #[test]
    fn reader_sees_writer_edit() {
        let (src, _out, _a, b) = pair();
        std::fs::write(src.path().join("a.md"), "# A\n\nzebrafish\n").unwrap();
        let (wiki, stale) = b.fresh();
        assert_eq!(stale, None);
        assert!(finds(&wiki, "zebrafish"));
        assert_eq!(b.compiles.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn reader_call_without_edits_is_fresh_and_fast() {
        let (_src, _out, a, b) = pair();
        let _ = a.fresh();
        let t = Instant::now();
        assert_eq!(b.fresh().1, None);
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[cfg(unix)]
    #[test]
    fn reader_gets_writer_compile_error() {
        use std::os::unix::fs::PermissionsExt;
        let (src, out, a, b) = pair();
        let _ = a.fresh();
        std::fs::set_permissions(out.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        std::fs::write(src.path().join("a.md"), "# A\n\nzebrafish\n").unwrap();
        let from_b = b.fresh().1;
        let from_a = a.fresh().1;
        std::fs::set_permissions(out.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(from_b, from_a);
        assert!(from_b.is_some_and(|e| e != RUNNING), "{from_a:?}");
    }

    #[test]
    fn writer_drop_hands_over() {
        let (src, _out, a, b) = pair();
        let _ = a.fresh();
        a.release_writer();
        std::fs::write(src.path().join("a.md"), "# A\n\nzebrafish\n").unwrap();
        let (wiki, stale) = b.fresh();
        assert_eq!(stale, None);
        assert!(finds(&wiki, "zebrafish"));
        assert!(b.is_writer());
    }

    #[test]
    fn reader_starts_on_empty_dir() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.md"), "# A\n\nalpha\n").unwrap();
        let out = tempfile::tempdir().unwrap();
        let (from, into) = (src.path().to_path_buf(), out.path().to_path_buf());
        let first =
            std::thread::spawn(move || Live::start(&from, &into, CompileOptions::default()));
        // `compile.lock` appears after the writer took `writer.lock`.
        let lock = out.path().join(BARRIER_DIR).join("compile.lock");
        let until = Instant::now() + Duration::from_secs(10);
        while !lock.exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        let second = Live::start(src.path(), out.path(), CompileOptions::default()).unwrap();
        assert!(!second.is_writer());
        let started = Instant::now();
        let stale = second.fresh().1;
        let took = started.elapsed();
        assert!(took < Duration::from_millis(1500), "{took:?}");
        assert!(
            stale.is_none() || stale.as_deref() == Some(RUNNING),
            "{stale:?}"
        );
        assert!(first.join().unwrap().unwrap().is_writer());
    }

    #[test]
    fn serve_dir_is_clean_after_calls() {
        let (_src, out, a, b) = pair();
        let _ = a.fresh();
        for _ in 0..20 {
            assert_eq!(b.fresh().1, None);
        }
        let mut names: Vec<String> = std::fs::read_dir(out.path().join(BARRIER_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["compile.lock", "writer.lock"]);
    }

    #[test]
    fn a_later_process_with_a_reused_pid_gets_a_new_id() {
        let first = server_id(0);
        std::thread::sleep(Duration::from_millis(2));
        assert_ne!(first, server_id(0));
    }

    #[test]
    fn reader_call_without_edits_keeps_its_snapshot() {
        let (_src, _out, a, b) = pair();
        let _ = a.fresh();
        let (one, _) = b.fresh();
        let (two, stale) = b.fresh();
        assert_eq!(stale, None);
        assert!(Arc::ptr_eq(&one, &two));
    }

    #[test]
    fn takeover_acks_every_waiting_barrier() {
        let (_src, out, a, b) = pair();
        let _ = a.fresh();
        a.release_writer();
        let serve = out.path().join(BARRIER_DIR);
        // Many, so `read_dir` order (hashed on APFS and ext4) is not token order.
        for token in 1..=12 {
            std::fs::write(serve.join(format!("barrier-1.1.0-{token}")), "").unwrap();
        }
        assert_eq!(b.fresh().1, None);
        for token in 1..=12 {
            assert!(serve.join(format!("ack-1.1.0-{token}")).exists(), "{token}");
        }
    }

    #[test]
    fn a_timed_out_call_forgets_its_token() {
        let (_src, _out, a, b) = pair();
        let _ = a.fresh();
        // The writer stays but never acks: every call times out.
        a.lock().compiling = true;
        b.lock().compiled_gen = 0;
        let (_, stale) = b.fresh();
        assert_eq!(stale.as_deref(), Some(RUNNING));
        // An ack that lands after the call gave up.
        let token = b.lock().barrier_sent;
        let ack = b
            .dir
            .join(BARRIER_DIR)
            .join(format!("ack-{}-{token}", b.id));
        std::fs::write(&ack, "").unwrap();
        std::thread::sleep(watch::DEBOUNCE * 2);
        assert_eq!(b.lock().acked, Vec::<u64>::new());
    }

    #[test]
    fn a_panicking_compile_is_an_error() {
        let r: Result<(), WikiError> = unpanicked(|| panic!("boom"));
        assert_eq!(r.unwrap_err().to_string(), "the compile panicked");
    }
}
