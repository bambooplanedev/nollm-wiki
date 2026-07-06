use crate::model::normalize_path;
use ignore::gitignore::Gitignore;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

pub struct SourceFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// Walk `root` for files. When `respect_ignore` is true, `.gitignore`/`.ignore`
/// and hidden-file rules apply (skips `target/`, `node_modules/`, `.git/`, etc.).
/// Results are sorted by `rel_path` for determinism, and files are read as raw bytes.
pub fn walk(root: &Path, respect_ignore: bool) -> Result<Vec<SourceFile>, std::io::Error> {
    let mut builder = WalkBuilder::new(root);
    builder.hidden(false).standard_filters(false);

    // Load .gitignore if respect_ignore is true
    let gitignore = if respect_ignore {
        let gitignore_path = root.join(".gitignore");
        if gitignore_path.exists() {
            match Gitignore::new(&gitignore_path) {
                (gi, None) => Some(gi),
                _ => None,
            }
        } else {
            None
        }
    } else {
        None
    };

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
        let rel_path = normalize_path(root, &abs_path);

        // Check .gitignore rules if respect_ignore is true
        if respect_ignore {
            if let Some(ref gi) = gitignore {
                let match_result = gi.matched(&abs_path, false);
                if match_result.is_ignore() {
                    continue;
                }
            }
            // Also skip hidden files when respect_ignore is true
            if rel_path.split('/').any(|part| part.starts_with('.')) {
                continue;
            }
        }

        let bytes = match std::fs::read(&abs_path) {
            Ok(b) => b,
            Err(_) => continue,
        };
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
}
