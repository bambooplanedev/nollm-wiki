//! The barrier protocol: a tool call writes a barrier file and waits for its ack; the writer scans for barriers and acks them.

use super::{Live, State, BARRIER_DIR, RUNNING, START_WAIT, WAIT};
use crate::query::Wiki;
use crate::watch;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

impl Live {
    /// Writer: note every barrier in `.serve` at least `min_age` old, for
    /// when their events were missed (a takeover, a lost event, before the
    /// watcher started). Removes barriers and acks older than any call waits.
    pub(super) fn scan_barriers(&self, s: &mut State, min_age: Duration) {
        let Ok(entries) = std::fs::read_dir(self.dir.join(BARRIER_DIR)) else {
            return;
        };
        let now = SystemTime::now();
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let age = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .unwrap_or_default();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if age > WAIT * 2
                && ["barrier-", "ack-", "tmp-ack-"]
                    .iter()
                    .any(|k| name.starts_with(k))
            {
                let _ = std::fs::remove_file(&path);
            } else if let Some(barrier) = self.foreign_barrier(&path).filter(|_| age >= min_age) {
                found.push((barrier, path));
            }
        }
        // In token order: a later token noted first would hide an earlier one.
        found.sort();
        for (barrier, path) in found {
            Self::note_barrier(s, barrier, &path);
        }
    }

    /// A new barrier file. Created under the lock, so tokens reach the
    /// watcher in token order; one file per token, so an event names exactly
    /// the barrier it closes.
    pub(super) fn put_barrier(&self, s: &mut State) -> (u64, PathBuf, std::io::Result<()>) {
        s.barrier_sent += 1;
        let token = s.barrier_sent;
        let barrier = self
            .dir
            .join(BARRIER_DIR)
            .join(format!("barrier-{}-{token}", self.id));
        let wrote = std::fs::write(&barrier, "");
        (token, barrier, wrote)
    }

    /// A reader's `fresh`: put a barrier, wait for the writer to ack it, and
    /// reload. `None` if this server took over as the writer meanwhile.
    pub(super) fn await_ack(&self, deadline: Instant) -> Option<(Arc<Wiki>, Option<String>)> {
        let mut s = self.lock();
        let end = if s.compiled_gen == 0 {
            deadline.min(Instant::now() + START_WAIT)
        } else {
            deadline
        };
        let (token, barrier, wrote) = self.put_barrier(&mut s);
        if let Err(e) = wrote {
            // No barrier, no ack: answer from disk at once.
            drop(s);
            let _ = self.reload(Some(end));
            return Some((self.wiki(), Some(format!("barrier: {e}"))));
        }
        let ack = self
            .dir
            .join(BARRIER_DIR)
            .join(format!("ack-{}-{token}", self.id));
        let acked = loop {
            let left = end.saturating_duration_since(Instant::now());
            s = self.wait(s, left.min(watch::DEBOUNCE), |s| !s.acked.contains(&token));
            // Its event may have been lost.
            if s.acked.contains(&token) || ack.exists() {
                s.acked.retain(|&t| t != token);
                break true;
            }
            drop(s);
            if self.try_become_writer(false) {
                let _ = std::fs::remove_file(&barrier);
                let _ = std::fs::remove_file(&ack);
                self.lock().acked.retain(|&t| t != token);
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
            let (compiled, reason) = match std::fs::read_to_string(&ack) {
                Ok(text) => {
                    let (compiled, reason) = text.split_once('\n').unwrap_or((&text, ""));
                    (Some(compiled.to_string()), reason.to_string())
                }
                Err(e) => (None, e.to_string()),
            };
            stale = Some(reason).filter(|r| !r.is_empty());
            let unchanged = compiled.is_some() && compiled == self.lock().loaded;
            match if unchanged {
                Some(Ok(()))
            } else {
                self.reload(Some(end))
            } {
                Some(Ok(())) => {
                    let mut s = self.lock();
                    s.compiled_gen = s.compiled_gen.max(1);
                    s.loaded = compiled;
                }
                Some(Err(e)) => stale = stale.or(Some(e)),
                // A compile holds the lock past the call: the next ack retries.
                None => stale = Some(RUNNING.to_string()),
            }
        }
        let _ = std::fs::remove_file(&barrier);
        let _ = std::fs::remove_file(&ack);
        // An ack event that came in after the wait ended.
        let mut s = self.lock();
        s.acked.retain(|&t| t != token);
        Some((Arc::clone(&s.wiki), stale))
    }

    /// `<id>-<token>` of `path` if it is another server's barrier in `dir`.
    /// Only a `<pid>.<nanos>.<n>` id counts: a 0.6.x `barrier-<pid>-<n>` has no
    /// reader left to remove its ack.
    pub(super) fn foreign_barrier(&self, path: &Path) -> Option<(String, u64)> {
        if path.parent()? != self.dir.join(BARRIER_DIR) {
            return None;
        }
        let (id, token) = path
            .file_name()?
            .to_str()?
            .strip_prefix("barrier-")?
            .rsplit_once('-')?;
        let token = token.parse().ok()?;
        let parts: Vec<&str> = id.split('.').collect();
        let server = parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        (server && id != self.id).then(|| (id.to_string(), token))
    }

    /// Writer: if `path`, another server's barrier `(id, token)`, is not
    /// yet seen, owe it an ack once a compile reaches the current `event_gen`.
    pub(super) fn note_barrier(s: &mut State, (id, token): (String, u64), path: &Path) {
        if s.foreign_seen.get(&id).is_some_and(|&t| t >= token) || !path.exists() {
            return;
        }
        s.foreign.push((format!("{id}-{token}"), s.event_gen));
        s.foreign_seen.insert(id, token);
    }

    /// Writer: ack every barrier the last compile covers. The ack holds a
    /// line naming that compile, then the stale reason, empty for none, and
    /// appears whole by a rename.
    pub(super) fn ack_ready(&self, s: &mut State) {
        let done = s.compiled_gen;
        let serve = self.dir.join(BARRIER_DIR);
        let reason = format!(
            "{} {done}\n{}",
            self.id,
            s.stale.as_deref().unwrap_or_default()
        );
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
}

/// The token of `path` if it is the server `id`'s `kind` file (`barrier` or
/// `ack`) in `dir`: `<kind>-<id>-<token>`. The id keeps servers sharing one
/// `--dir` from closing each other's barriers.
pub(super) fn own_token(dir: &Path, kind: &str, id: &str, path: &Path) -> Option<u64> {
    if path.parent()? != dir.join(BARRIER_DIR) {
        return None;
    }
    path.file_name()?
        .to_str()?
        .strip_prefix(&format!("{kind}-{id}-"))?
        .parse()
        .ok()
}
