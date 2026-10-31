//! Watch mode (`wiki compile --watch`): recompile on source change, ignoring events from the output directory.

use crate::formats::Registry;
use crate::model::normalize_path;
use crate::{compile, CompileOptions, WikiError};
use notify::{RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

/// What the last compile's walk visited. Lets a filesystem event be judged
/// against the same ignore rules the compile applied, without re-reading them:
/// a path under a pruned tree (`target/`, `.git/`, hidden dirs) has no visited
/// parent and so never counts.
pub struct WatchSet {
    root: PathBuf,
    files: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
    /// Memory folders, which need not exist yet: anything under one counts.
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
    /// and only if a handled file, an ignore file, or a visible directory.
    pub fn relevant(&self, path: &Path) -> bool {
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
        self.registry.handles(&normalize_path(&self.root, path))
            || name == ".gitignore"
            || name == ".ignore"
            || (path.is_dir() && !name.starts_with('.'))
    }
}

fn recompile_once(input: &Path, output: &Path, opts: &CompileOptions) -> Result<(), WikiError> {
    let r = compile(input, output, opts)?;
    eprintln!(
        "recompiled: {} pages ({} written)",
        r.pages_total, r.pages_written
    );
    Ok(())
}

/// How long to let a burst of events settle before acting on it.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// Watch each `(path, mode)` and hand every event to `on_event`, which runs on
/// notify's own thread. The caller must keep the returned watcher alive.
pub fn start(
    paths: &[(&Path, RecursiveMode)],
    on_event: impl FnMut(notify::Result<notify::Event>) + Send + 'static,
) -> Result<notify::RecommendedWatcher, WikiError> {
    let mut watcher =
        notify::recommended_watcher(on_event).map_err(|e| WikiError::Watch(e.to_string()))?;
    for (path, mode) in paths {
        watcher
            .watch(path, *mode)
            .map_err(|e| WikiError::Watch(e.to_string()))?;
    }
    Ok(watcher)
}

pub fn watch(input: &Path, output: &Path, opts: &CompileOptions) -> Result<(), WikiError> {
    recompile_once(input, output, opts)?;

    let (tx, rx) = channel();
    let _watcher = start(&[(input, RecursiveMode::Recursive)], move |res| {
        let _ = tx.send(res);
    })?;

    eprintln!("watching {} … (Ctrl-C to stop)", input.display());
    while let Ok(res) = rx.recv() {
        // A recompile writes pages into `output`; ignore events that only touch
        // the output tree so we never trigger ourselves into a busy loop.
        let mut relevant = event_is_relevant(&res, output);
        // Debounce: drain any events that arrived during compilation.
        std::thread::sleep(DEBOUNCE);
        while let Ok(res) = rx.try_recv() {
            relevant |= event_is_relevant(&res, output);
        }
        if !relevant {
            continue;
        }
        if let Err(e) = recompile_once(input, output, opts) {
            eprintln!("recompile error: {e}");
        }
    }
    Ok(())
}

/// False for a mere read: inotify reports every open, and a compile's own
/// walk opens every source file, so counting reads would recompile forever.
pub(crate) fn is_change(ev: &notify::Event) -> bool {
    !matches!(ev.kind, notify::EventKind::Access(_))
}

/// True if a watch event changes at least one path outside `output` — i.e. a
/// real source change, not our own generated pages. Watch errors are treated
/// as relevant so a real change is never missed.
fn event_is_relevant(res: &notify::Result<notify::Event>, output: &Path) -> bool {
    match res {
        Ok(ev) => is_change(ev) && ev.paths.iter().any(|p| !crate::is_under(p, output)),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn start_delivers_events_for_a_write() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let _w = start(&[(&root, RecursiveMode::Recursive)], move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        let file = root.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let ev = rx.recv_timeout(left).expect("no event within 5s").unwrap();
            if ev
                .paths
                .iter()
                .any(|p| p.canonicalize().ok().as_ref() == Some(&file))
            {
                break;
            }
        }
    }

    #[test]
    fn recompile_once_writes_pages() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("raw");
        let output = dir.path().join("out");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("a.txt"), "# A\n\nbody\n").unwrap();
        let opts = crate::CompileOptions {
            incremental: true,
            ..Default::default()
        };
        recompile_once(&input, &output, &opts).unwrap();
        assert!(output.join("a.md").exists());
    }

    #[test]
    fn event_relevance_ignores_output_only_events() {
        use std::path::{Path, PathBuf};
        let output = Path::new("/proj/wiki");
        let generated = notify::Event::new(notify::EventKind::Any)
            .add_path(PathBuf::from("/proj/wiki/alpha.md"));
        let source =
            notify::Event::new(notify::EventKind::Any).add_path(PathBuf::from("/proj/alpha.txt"));
        assert!(!event_is_relevant(&Ok(generated), output));
        assert!(event_is_relevant(&Ok(source), output));
    }

    #[test]
    fn opening_a_source_file_is_not_relevant() {
        use notify::event::{AccessKind, AccessMode, ModifyKind};
        let output = Path::new("/proj/wiki");
        let file = PathBuf::from("/proj/src/a.rs");
        let open = notify::Event::new(notify::EventKind::Access(AccessKind::Open(AccessMode::Any)))
            .add_path(file.clone());
        let modify = notify::Event::new(notify::EventKind::Modify(ModifyKind::Any)).add_path(file);
        assert!(!event_is_relevant(&Ok(open), output));
        assert!(event_is_relevant(&Ok(modify), output));
    }

    /// Compile a tempdir and return its (canonical root, watch set).
    fn compiled() -> (tempfile::TempDir, PathBuf, WatchSet) {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let input = root.join("proj");
        for d in ["src", "docs", "target/debug"] {
            std::fs::create_dir_all(input.join(d)).unwrap();
        }
        std::fs::write(input.join("src/a.rs"), "pub fn a() {}\n").unwrap();
        std::fs::write(input.join("docs/n.md"), "# N\n\nbody\n").unwrap();
        std::fs::write(input.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(input.join("target/debug/x.rs"), "pub fn x() {}\n").unwrap();
        let r = compile(&input, &root.join("out"), &CompileOptions::default()).unwrap();
        (dir, input, r.watch_set)
    }

    #[test]
    fn walked_source_file_is_relevant() {
        let (_d, r, w) = compiled();
        assert!(w.relevant(&r.join("src/a.rs")));
    }

    #[test]
    fn new_handled_file_in_visited_dir_is_relevant() {
        let (_d, r, w) = compiled();
        assert!(w.relevant(&r.join("src/b.rs")));
    }

    #[test]
    fn unhandled_file_is_not_relevant() {
        let (_d, r, w) = compiled();
        assert!(!w.relevant(&r.join("src/a.o")));
    }

    #[test]
    fn ignored_tree_events_are_not_relevant() {
        let (_d, r, w) = compiled();
        std::fs::create_dir(r.join("target/debug/new_dir")).unwrap();
        assert!(!w.relevant(&r.join("target/debug/y.rs")));
        assert!(!w.relevant(&r.join("target/debug/new_dir")));
    }

    #[test]
    fn gitignore_change_is_relevant() {
        let (_d, r, w) = compiled();
        assert!(w.relevant(&r.join(".gitignore")));
    }

    #[test]
    fn new_visible_subdir_is_relevant() {
        let (_d, r, w) = compiled();
        std::fs::create_dir(r.join("src/sub")).unwrap();
        assert!(w.relevant(&r.join("src/sub")));
    }

    #[test]
    fn hidden_new_dir_is_not_relevant() {
        let (_d, r, w) = compiled();
        std::fs::create_dir(r.join(".cache")).unwrap();
        assert!(!w.relevant(&r.join(".cache")));
    }

    #[test]
    fn deleted_source_file_is_relevant() {
        let (_d, r, w) = compiled();
        std::fs::remove_file(r.join("src/a.rs")).unwrap();
        assert!(w.relevant(&r.join("src/a.rs")));
    }

    #[test]
    fn repo_memory_is_relevant_before_it_exists() {
        let r = PathBuf::from("/proj");
        let w = WatchSet::new(&r, [], [r.clone()]).with_memory([r.join(".agents/memory")]);
        assert!(w.relevant(&r.join(".agents")));
        assert!(w.relevant(&r.join(".agents/memory")));
        assert!(w.relevant(&r.join(".agents/memory/new.md")));
        assert!(w.relevant(&r.join(".agents/memory/sub/deep.md")));
        assert!(!w.relevant(&r.join(".agents/other.md")));
        assert!(!w.relevant(&r.join(".cache")));
    }

    #[test]
    fn a_sibling_of_a_missing_memory_folder_is_not_relevant() {
        let slot = PathBuf::from("/c/projects/-proj");
        let w = WatchSet::new(Path::new("/proj"), [], [PathBuf::from("/proj")])
            .with_memory([slot.join("memory")]);
        assert!(!w.relevant(&slot.join("abc.jsonl")));
        assert!(w.relevant(&slot.join("memory")));
        assert!(w.relevant(&slot.join("memory/new.md")));
    }

    #[test]
    fn external_memory_files_are_relevant() {
        let (d, _r, w) = compiled();
        let ext = d.path().canonicalize().unwrap().join("ext");
        let w = w.with_memory([ext.clone()]);
        assert!(w.relevant(&ext.join("n.md")));
        assert!(!w.relevant(&d.path().join("elsewhere/n.md")));
    }
}
