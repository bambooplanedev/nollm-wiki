//! Source tree walk: gitignore-aware, sorted, deterministic file discovery for the compiler.

use crate::model::normalize_path;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

pub struct SourceFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// What a walk reached: the kept files, and every directory it yielded (root
/// included, sorted). Ignored subtrees are pruned, so they appear in neither.
pub struct Walked {
    pub files: Vec<SourceFile>,
    pub dirs: Vec<PathBuf>,
}

/// Walk `root` for files. When `respect_ignore` is true, `.gitignore`/`.ignore`
/// rules (including nested `.gitignore` files, global excludes, and
/// `.git/info/exclude`), hidden-file rules, and directory-subtree pruning all
/// apply — so `target/`, `node_modules/`, `.git/`, and any directory excluded
/// by a matching rule are skipped entirely (their contents are never visited,
/// not merely filtered file-by-file). `require_git(false)` ensures these
/// rules are honored even when `root` is not inside an actual `.git`
/// repository (e.g. a plain tempdir in tests). When `respect_ignore` is
/// false, no filtering is applied and all files are returned, including
/// hidden and normally-ignored ones.
/// Results are sorted by `rel_path` for determinism, and files are read as
/// raw bytes. Entries that error out (permission issues, broken symlinks,
/// unreadable files) are skipped rather than aborting the walk.
///
/// `keep` decides, from the relative path alone, whether a file is worth
/// reading. It is consulted *before* the file is opened, which is the whole
/// point: see the note at the read site below.
pub fn walk(
    root: &Path,
    respect_ignore: bool,
    keep: &dyn Fn(&str) -> bool,
) -> Result<Walked, std::io::Error> {
    Ok(walk_with(
        root,
        |b| {
            b.hidden(respect_ignore)
                .git_ignore(respect_ignore)
                .git_global(respect_ignore)
                .git_exclude(respect_ignore)
                .ignore(respect_ignore)
                .parents(respect_ignore);
        },
        keep,
    ))
}

fn walk_with(
    root: &Path,
    rules: impl FnOnce(&mut WalkBuilder),
    keep: &dyn Fn(&str) -> bool,
) -> Walked {
    let mut builder = WalkBuilder::new(root);
    rules(&mut builder);
    builder.require_git(false);
    // Symlinks are intentionally NOT followed (follow_links defaults to false):
    // following them risks cycles, escaping the input tree, and machine-
    // dependent ordering — all of which would break output determinism.

    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for result in builder.build() {
        // Skip unreadable entries, never abort the walk.
        let Ok(entry) = result else { continue };
        let is_file = match entry.file_type() {
            Some(ft) => ft.is_file(),
            // Type unknown from the walk's own stat: take a fresh stat rather
            // than silently dropping a possibly-regular file. This does not
            // change the deliberate no-follow policy for known symlinks.
            None => std::fs::metadata(entry.path()).is_ok_and(|m| m.is_file()),
        };
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            dirs.push(entry.path().to_path_buf());
        }
        if !is_file {
            continue;
        }
        let abs_path = entry.path().to_path_buf();
        let rel_path = normalize_path(root, &abs_path);
        // Decide from the path BEFORE opening the file. `fs::read` allocates
        // the entire file, and every `SourceFile` stays resident until the
        // caller is done, so peak memory is the sum of everything read here.
        // Deciding afterwards (in `Registry::extract`, on the extension) is
        // too late: one stray 20GB video sitting in the input tree costs 20GB
        // of RSS before anything ever looks at its name.
        if !keep(&rel_path) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&abs_path) else {
            continue;
        };
        files.push(SourceFile {
            abs_path,
            rel_path,
            bytes,
        });
    }

    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    dirs.sort();
    Walked { files, dirs }
}

/// Walk a memory folder: every ignore rule off (a memory folder is usually
/// git-excluded on purpose), hidden files skipped, `memory.md` (any case) left
/// out because it is the folder's own index. Sorted like [`walk`].
pub fn walk_memory(dir: &Path, keep: &dyn Fn(&str) -> bool) -> Result<Walked, std::io::Error> {
    Ok(walk_with(
        dir,
        |b| {
            b.hidden(true)
                .git_ignore(false)
                .git_global(false)
                .git_exclude(false)
                .ignore(false)
                .parents(false);
        },
        &|p| {
            !Path::new(p)
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("memory.md"))
                && keep(p)
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Read every file the walk reaches: these tests exercise walk's own
    /// traversal rules, not the caller's relevance filter.
    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn walk_returns_sorted_relative_files_and_honors_gitignore() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("b.txt"), b"bbb").unwrap();
        fs::write(root.join("a.txt"), b"aaa").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/c.txt"), b"ccc").unwrap();
        fs::write(root.join(".gitignore"), b"ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), b"nope").unwrap();

        let files = walk(root, true, &all).unwrap().files;
        let rels: Vec<_> = files.iter().map(|f| f.rel_path.clone()).collect();
        assert_eq!(rels, vec!["a.txt", "b.txt", "sub/c.txt"]);
        assert_eq!(files[0].bytes, b"aaa");
    }

    #[test]
    fn walk_no_ignore_includes_gitignored() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), b"ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), b"nope").unwrap();
        let files = walk(root, false, &all).unwrap().files;
        assert!(files.iter().any(|f| f.rel_path == "ignored.txt"));
    }

    #[test]
    fn walk_prunes_whole_ignored_directory_subtrees() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), b"target/\n").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/foo.o"), b"binary").unwrap();
        fs::write(root.join("keep.txt"), b"keep").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main").unwrap();

        let files = walk(root, true, &all).unwrap().files;
        let rels: Vec<_> = files.iter().map(|f| f.rel_path.clone()).collect();

        assert!(rels.contains(&"keep.txt".to_string()));
        assert!(
            !rels.iter().any(|r| r.starts_with("target/")),
            "target/ subtree should be pruned entirely, got: {rels:?}"
        );
        assert!(
            !rels.iter().any(|r| r.starts_with(".git/")),
            ".git/ should be excluded as a hidden directory, got: {rels:?}"
        );
    }

    /// Regression: a compile once held every byte of the input tree in memory,
    /// including files no extractor could ever handle. An input directory that
    /// happened to sit next to ~48GB of raw `.mov` footage took ~48GB of RSS
    /// and never finished. `keep` has to reject on the path, before the read —
    /// so the bytes are not merely dropped later, they are never allocated.
    #[test]
    fn unhandled_files_are_not_read_into_memory() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("notes.txt"), b"small").unwrap();
        // Stands in for the video: big enough that holding it would show up.
        fs::write(root.join("footage.mov"), vec![0u8; 8 * 1024 * 1024]).unwrap();

        let files = walk(root, true, &|p| {
            Path::new(p).extension() == Some("txt".as_ref())
        })
        .unwrap()
        .files;

        let rels: Vec<_> = files.iter().map(|f| f.rel_path.clone()).collect();
        assert_eq!(rels, vec!["notes.txt"]);
        let held: usize = files.iter().map(|f| f.bytes.len()).sum();
        assert_eq!(
            held, 5,
            "walk retained bytes for a file it was told to skip: {held}"
        );
    }

    #[test]
    fn walk_memory_ignores_git_rules_but_skips_hidden_and_index_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), b"a.md\n").unwrap();
        fs::write(root.join("a.md"), b"a").unwrap();
        fs::write(root.join("MEMORY.md"), b"i").unwrap();
        fs::write(root.join("memory.md"), b"i").unwrap();
        fs::write(root.join("._a.md"), b"j").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/b.md"), b"b").unwrap();

        let w = walk_memory(root, &all).unwrap();
        let rels: Vec<_> = w.files.iter().map(|f| f.rel_path.clone()).collect();
        assert_eq!(rels, vec!["a.md", "sub/b.md"]);
        assert_eq!(w.dirs, vec![root.to_path_buf(), root.join("sub")]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("real.txt"), b"real").unwrap();
        symlink(root.join("real.txt"), root.join("link.txt")).unwrap();

        let files = walk(root, true, &all).unwrap().files;
        let rels: Vec<_> = files.iter().map(|f| f.rel_path.clone()).collect();
        assert!(
            rels.contains(&"real.txt".to_string()),
            "real file missing: {rels:?}"
        );
        assert!(
            !rels.contains(&"link.txt".to_string()),
            "symlink was followed (determinism risk): {rels:?}"
        );
    }
}
