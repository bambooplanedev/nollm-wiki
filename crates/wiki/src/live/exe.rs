//! The running executable and its replacement check: `EXE` notices when the binary is swapped for a new version.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::SystemTime;

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
    pub(super) fn of(paths: impl IntoIterator<Item = PathBuf>) -> Exe {
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
