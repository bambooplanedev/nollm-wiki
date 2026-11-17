//! Source tree walk: gitignore-aware, sorted, deterministic file discovery.
//!
//! The contract is [`walk`], [`Walked`], [`SourceFile`], [`SKIPPED_DIRS`] and
//! [`pruned_dir`]; [`skipped`] and [`walk_memory`] serve the wiki compiler
//! only and may change with it.

use crate::formats::code::{ext_of, shell_shebang};
use crate::support::{is_under, normalize_path};
use ignore::{DirEntry, WalkBuilder};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct SourceFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// Folders the compile never walks: the superpowers workflow's process
/// artefacts (specs, plans, ledgers, run logs; memory notes carry what they
/// concluded) and vendored code. On one bench project they were 2,358 of
/// 3,298 pages and 77% of the wiki's bytes (2,261 run logs, plans and specs
/// under `docs/superpowers/`, 97 CLI11 pages under `third_party/`).
/// `--no-ignore` walks them too.
pub const SKIPPED_DIRS: [&str; 6] = [
    "superpowers",
    "third_party",
    "third-party",
    "vendor",
    "vendored",
    "node_modules",
];

/// Whether the compile prunes a directory of this name: a [`SKIPPED_DIRS`]
/// name, or a hidden one (a leading `.`, `WalkBuilder::hidden`'s rule, which
/// an ignore-file whitelist such as `!.github/` overrides).
pub fn pruned_dir(name: &str) -> bool {
    name.starts_with('.') || SKIPPED_DIRS.contains(&name)
}

/// What a walk reached: the kept files, and every directory it yielded (root
/// included, sorted). Ignored subtrees are pruned, so they appear in neither.
pub struct Walked {
    pub files: Vec<SourceFile>,
    pub dirs: Vec<PathBuf>,
    /// Relative paths, sorted, of files that passed `keep` but could not be read.
    pub unreadable: Vec<String>,
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
/// hidden and normally-ignored ones. [`SKIPPED_DIRS`] are pruned with the
/// ignore rules (the root itself is never pruned by name).
/// Results are sorted by `rel_path` for determinism, and files are read as
/// raw bytes. Entries that error out (permission issues, broken symlinks,
/// unreadable files) are skipped rather than aborting the walk.
///
/// `keep` decides, from the relative path alone, whether a file is worth
/// reading. It is consulted *before* the file is opened, which is the whole
/// point: see the note at the read site below.
pub fn walk(root: &Path, respect_ignore: bool, keep: &dyn Fn(&str) -> bool) -> Walked {
    walk_with(root, |b| project_rules(b, respect_ignore), keep)
}

fn project_rules(b: &mut WalkBuilder, respect_ignore: bool) {
    b.hidden(respect_ignore)
        .git_ignore(respect_ignore)
        .git_global(respect_ignore)
        .git_exclude(respect_ignore)
        .ignore(respect_ignore)
        .parents(respect_ignore);
    if respect_ignore {
        b.filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_some_and(|t| t.is_dir())
                || !e
                    .file_name()
                    .to_str()
                    .is_some_and(|n| SKIPPED_DIRS.contains(&n))
        });
    }
}

/// How many files [`walk`] leaves out of `root`: `(name, files)` per
/// [`SKIPPED_DIRS`] name, then `("ignored", files)` for every other rule, zero
/// counts dropped. A file counts once, for its outermost pruned path, so
/// `target/vendor/x` is ignored and a gitignored `vendor/` is `vendor`. A file
/// `keep` refuses was reached, so it is no skip. `.git` and the `compiled`
/// folders (the output, the memory folders) are left out of the count. Lists
/// directories and opens no source file; a second walk, so the CLI's alone.
pub fn skipped(root: &Path, compiled: &[PathBuf]) -> Vec<(&'static str, usize)> {
    let mut reached = WalkBuilder::new(root);
    project_rules(&mut reached, true);
    reached.require_git(false);
    let reached: HashSet<PathBuf> = reached.build().flatten().map(DirEntry::into_path).collect();

    let compiled = compiled.to_vec();
    let mut everything = WalkBuilder::new(root);
    everything.standard_filters(false).filter_entry(move |e| {
        e.file_name().to_str() != Some(".git")
            && !(e.file_type().is_some_and(|t| t.is_dir())
                && compiled.iter().any(|d| is_under(e.path(), d)))
    });

    let mut counts = [0; SKIPPED_DIRS.len() + 1];
    for entry in everything.build().flatten() {
        let path = entry.path();
        if !entry.file_type().is_some_and(|t| t.is_file()) || reached.contains(path) {
            continue;
        }
        let outermost = path
            .ancestors()
            .take_while(|a| *a != root)
            .filter(|a| !reached.contains(*a))
            .last();
        let rule = outermost
            .filter(|top| *top != path)
            .and_then(|top| top.file_name()?.to_str())
            .and_then(|n| SKIPPED_DIRS.iter().position(|s| *s == n));
        counts[rule.unwrap_or(SKIPPED_DIRS.len())] += 1;
    }
    SKIPPED_DIRS
        .into_iter()
        .chain(["ignored"])
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .collect()
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
    let mut unreadable = Vec::new();
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
        // Deciding afterwards (when the caller extracts, on the extension) is
        // too late: one stray 20GB video sitting in the input tree costs 20GB
        // of RSS before anything ever looks at its name.
        if !keep(&rel_path) && !shell_script(&abs_path, &rel_path, keep) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&abs_path) else {
            unreadable.push(rel_path);
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
    unreadable.sort();
    Walked {
        files,
        dirs,
        unreadable,
    }
}

/// Whether a file `keep` refused by its name is a shell script it would keep
/// as `<name>.sh`: the name has no extension (`pre-commit`) and the first line
/// is a shell shebang (`shell_shebang`). Reads only that line's first bytes,
/// and only from a regular file: a FIFO would block the open, and the walk
/// never follows a symlink.
pub fn shell_script(abs_path: &Path, rel_path: &str, keep: &dyn Fn(&str) -> bool) -> bool {
    use std::io::Read;
    let mut head = Vec::new();
    ext_of(rel_path).is_empty()
        && keep(&format!("{rel_path}.sh"))
        && std::fs::symlink_metadata(abs_path).is_ok_and(|m| m.is_file())
        && std::fs::File::open(abs_path)
            .and_then(|f| f.take(128).read_to_end(&mut head))
            .is_ok()
        && shell_shebang(&head)
}

/// Walk a memory folder: every ignore rule off (a memory folder is usually
/// git-excluded on purpose), hidden files skipped, `memory.md` (any case) left
/// out because it is the folder's own index. Sorted like [`walk`].
pub fn walk_memory(dir: &Path, keep: &dyn Fn(&str) -> bool) -> Walked {
    walk_with(
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
    )
}

#[cfg(test)]
mod tests;
