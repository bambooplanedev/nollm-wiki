//! MCP server for a compiled wiki (`wiki serve`): exposes search/neighbors/
//! lint as tools and pages as resources over stdio.

use crate::query::Wiki;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Identity of the compiled output, cheap to compute: `index.json`'s
/// (mtime, len). A recompile rewrites index.json, changing at least one.
#[derive(PartialEq, Clone, Copy, Debug)]
struct Fingerprint {
    mtime: SystemTime,
    len: u64,
}

fn fingerprint(dir: &Path) -> std::io::Result<Fingerprint> {
    let meta = std::fs::metadata(dir.join("index.json"))?;
    Ok(Fingerprint {
        mtime: meta.modified()?,
        len: meta.len(),
    })
}

struct Loaded {
    wiki: Wiki,
    fingerprint: Fingerprint,
}

/// A lazily-reloading handle to a compiled wiki. Each access compares the
/// current `index.json` fingerprint against the loaded snapshot's and
/// reloads on change. A failed reload (mid-compile write, malformed index,
/// deleted file) keeps serving the previous snapshot; the reload is retried
/// on the next access.
pub struct WikiState {
    dir: PathBuf,
    inner: Mutex<Loaded>,
}

impl WikiState {
    pub fn load(dir: &Path) -> std::io::Result<WikiState> {
        let fp = fingerprint(dir)?;
        let wiki = Wiki::load(dir)?;
        Ok(WikiState {
            dir: dir.to_path_buf(),
            inner: Mutex::new(Loaded {
                wiki,
                fingerprint: fp,
            }),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn with_wiki<T>(&self, f: impl FnOnce(&Wiki) -> T) -> T {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(current) = fingerprint(&self.dir) {
            if current != guard.fingerprint {
                if let Ok(wiki) = Wiki::load(&self.dir) {
                    *guard = Loaded {
                        wiki,
                        fingerprint: current,
                    };
                }
                // Reload failure: keep old snapshot, retry next call.
            }
        }
        f(&guard.wiki)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{compile, CompileOptions};

    fn fixture(files: usize) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path().join("raw");
        let out = tmp.path().join("out");
        crate::generator::generate_corpus(&raw, files, 42).unwrap();
        compile(&raw, &out, &CompileOptions::default()).unwrap();
        (tmp, out)
    }

    #[test]
    fn load_fails_without_index_json() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(WikiState::load(tmp.path()).is_err());
    }

    #[test]
    fn with_wiki_serves_loaded_snapshot() {
        let (_tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        let n = state.with_wiki(|w| w.list_pages().len());
        assert_eq!(n, 3);
    }

    #[test]
    fn with_wiki_reloads_after_recompile() {
        let (tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        assert_eq!(state.with_wiki(|w| w.list_pages().len()), 3);

        // Recompile with more source files -> index.json changes (len differs).
        let raw = tmp.path().join("raw");
        crate::generator::generate_corpus(&raw, 5, 42).unwrap();
        compile(&raw, &out, &CompileOptions::default()).unwrap();

        assert_eq!(state.with_wiki(|w| w.list_pages().len()), 5);
    }

    #[test]
    fn with_wiki_keeps_old_snapshot_when_reload_fails() {
        let (_tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        assert_eq!(state.with_wiki(|w| w.list_pages().len()), 3);

        // Corrupt index.json (fingerprint changes, load will fail).
        std::fs::write(out.join("index.json"), "{ not json").unwrap();

        // Old snapshot still serves.
        assert_eq!(state.with_wiki(|w| w.list_pages().len()), 3);
    }
}
