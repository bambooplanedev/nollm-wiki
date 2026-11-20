//! What `serve --source` needs to watch a project: the watcher, its debounce, and which events count.

use crate::formats::Registry;
use crate::model::normalize_path;
use crate::WikiError;
use notify::{RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What the last compile's walk visited. Lets a filesystem event be judged
/// against the same ignore rules the compile applied, without re-reading them:
/// a path under a pruned tree (`target/`, `.git/`, hidden dirs, `SKIPPED_DIRS`) has no visited
/// parent and so never counts.
pub struct WatchSet {
    root: PathBuf,
    files: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
    /// Memory folders, which need not exist yet: anything under one counts,
    /// except its `logs/`, which the compile skips.
    memory: Vec<PathBuf>,
    registry: Registry,
}

impl WatchSet {
    pub fn new(
        root: &Path,
        files: impl IntoIterator<Item = PathBuf>,
        dirs: impl IntoIterator<Item = PathBuf>,
    ) -> WatchSet {
        WatchSet {
            root: root.to_path_buf(),
            files: files.into_iter().collect(),
            dirs: dirs.into_iter().collect(),
            memory: Vec::new(),
            registry: Registry::with_defaults(),
        }
    }

    /// Also count anything under `roots`, and the creation of a root's parent
    /// (`.agents` for the repo's `.agents/memory`): a hidden tree the walk
    /// never visits, so a first note there is otherwise invisible.
    #[must_use]
    pub fn with_memory(mut self, roots: impl IntoIterator<Item = PathBuf>) -> WatchSet {
        self.memory.extend(roots);
        self
    }

    /// Would a change at `path` (absolute, canonical) alter what a compile
    /// produces? Known paths always; new ones only inside a visited directory,
    /// and only if a handled file (or a shell script with no extension), an
    /// ignore file, or a visible directory.
    pub fn relevant(&self, path: &Path) -> bool {
        if self
            .memory
            .iter()
            .any(|m| crate::is_under(path, &m.join("logs")))
        {
            return false;
        }
        if self.files.contains(path) || self.dirs.contains(path) {
            return true;
        }
        if self
            .memory
            .iter()
            .any(|m| crate::is_under(path, m) || m.parent() == Some(path))
        {
            return true;
        }
        if !path.parent().is_some_and(|p| self.dirs.contains(p)) {
            return false;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let rel = normalize_path(&self.root, path);
        let handles = |p: &str| self.registry.handles(p);
        handles(&rel)
            || crate::walk::shell_script(path, &rel, &handles)
            || name == ".gitignore"
            || name == ".ignore"
            || (path.is_dir()
                && !name.starts_with('.')
                && !crate::walk::SKIPPED_DIRS.contains(&name))
    }
}

/// How long to let a burst of events settle before acting on it.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// Watch each `(path, mode)` and hand every event to `on_event`, which runs on
/// notify's own thread. The caller must keep the returned watcher alive.
/// All paths go in one batch: on macOS each `watch` call restarts the
/// `FSEvents` stream, and each start is a round trip to `fseventsd`, which
/// serves every process in turn.
pub fn start(
    paths: &[(&Path, RecursiveMode)],
    on_event: impl FnMut(notify::Result<notify::Event>) + Send + 'static,
) -> Result<notify::RecommendedWatcher, WikiError> {
    let err = |e: notify::Error| WikiError::Watch(e.to_string());
    let mut watcher = notify::recommended_watcher(on_event).map_err(err)?;
    let mut batch = watcher.paths_mut();
    for (path, mode) in paths {
        batch.add(path, *mode).map_err(err)?;
    }
    batch.commit().map_err(err)?;
    Ok(watcher)
}

/// False for a mere read: inotify reports every open, and a compile's own
/// walk opens every source file, so counting reads would recompile forever.
pub(crate) fn is_change(ev: &notify::Event) -> bool {
    !matches!(ev.kind, notify::EventKind::Access(_))
}

#[cfg(test)]
mod tests;
