use crate::model::normalize_path;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

pub struct SourceFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub bytes: Vec<u8>,
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
pub fn walk(root: &Path, respect_ignore: bool) -> Result<Vec<SourceFile>, std::io::Error> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(respect_ignore)
        .git_ignore(respect_ignore)
        .git_global(respect_ignore)
        .git_exclude(respect_ignore)
        .ignore(respect_ignore)
        .parents(respect_ignore)
        .require_git(false);

    let mut files = Vec::new();
    for result in builder.build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue, // skip unreadable entries, never abort the walk
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let abs_path = entry.path().to_path_buf();
        let bytes = match std::fs::read(&abs_path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let rel_path = normalize_path(root, &abs_path);
        files.push(SourceFile {
            abs_path,
            rel_path,
            bytes,
        });
    }

    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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

        let files = walk(root, true).unwrap();
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
        let files = walk(root, false).unwrap();
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

        let files = walk(root, true).unwrap();
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
}
