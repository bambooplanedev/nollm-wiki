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
    loop {
        match rx.recv() {
            Ok(_) => {
                // Debounce: drain any events that arrived during compilation.
                std::thread::sleep(Duration::from_millis(150));
                while rx.try_recv().is_ok() {}
                if let Err(e) = recompile_once(input, output, opts) {
                    eprintln!("recompile error: {e}");
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
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
}
