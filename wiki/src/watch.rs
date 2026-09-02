//! Watch mode (`wiki compile --watch`): recompile on source change, ignoring events from the output directory.

use crate::{compile, CompileOptions, WikiError};
use notify::{RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::channel;
use std::time::Duration;

pub fn recompile_once(input: &Path, output: &Path, opts: &CompileOptions) -> Result<(), WikiError> {
    let r = compile(input, output, opts)?;
    eprintln!(
        "recompiled: {} pages ({} written)",
        r.pages_total, r.pages_written
    );
    Ok(())
}

pub fn watch(input: &Path, output: &Path, opts: &CompileOptions) -> Result<(), WikiError> {
    recompile_once(input, output, opts)?;

    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| WikiError::Pool(e.to_string()))?;
    watcher
        .watch(input, RecursiveMode::Recursive)
        .map_err(|e| WikiError::Pool(e.to_string()))?;

    eprintln!("watching {} … (Ctrl-C to stop)", input.display());
    while let Ok(res) = rx.recv() {
        // A recompile writes pages into `output`; ignore events that only touch
        // the output tree so we never trigger ourselves into a busy loop.
        let mut relevant = event_is_relevant(&res, output);
        // Debounce: drain any events that arrived during compilation.
        std::thread::sleep(Duration::from_millis(150));
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

/// True if a watch event touches at least one path outside `output` — i.e. a
/// real source change, not our own generated pages. Watch errors are treated
/// as relevant so a real change is never missed.
fn event_is_relevant(res: &notify::Result<notify::Event>, output: &Path) -> bool {
    match res {
        Ok(ev) => ev.paths.iter().any(|p| !crate::is_under(p, output)),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
}
