//! Where agent memory lives: the repo's own folder, and the folders agents keep outside it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where a memory note came from: written on this machine, or committed to the repo.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    /// An agent folder, or a repo note git does not track (or no git at all).
    #[default]
    Local,
    /// A note in the repo's `.agents/memory` that git tracks: the name of the
    /// last commit's author (`None` when the history does not reach it), and
    /// whether the file differs from `HEAD`.
    Repo {
        author: Option<String>,
        edited: bool,
        /// The author's email is the compiling person's `git config user.email`.
        mine: bool,
    },
}

impl Origin {
    /// `local`, or `repo` then `, <author>`, `, you` (the person's own
    /// note) and `, edited` when they apply: `repo, Jane Doe, you, edited`.
    pub fn label(&self) -> String {
        match self {
            Origin::Local => "local".into(),
            Origin::Repo {
                author,
                edited,
                mine,
            } => {
                let mut s = String::from("repo");
                if let Some(a) = author {
                    s += ", ";
                    s += a;
                }
                if *mine {
                    s += ", you";
                }
                if *edited {
                    s += ", edited";
                }
                s
            }
        }
    }

    /// How far to trust the note: `local` 1.0, the person's own or edited
    /// `repo` note 0.8, any other `repo` note 0.5.
    pub fn trust(&self) -> f64 {
        match self {
            Origin::Local => 1.0,
            Origin::Repo {
                mine: false,
                edited: false,
                ..
            } => 0.5,
            Origin::Repo { .. } => 0.8,
        }
    }
}

/// Project memory kept inside the checkout.
pub const REPO_DIR: &str = ".agents/memory";

/// Claude Code's project folder name for `path`: every UTF-16 code unit that
/// is not an ASCII letter or digit becomes `-`; past 200 units the name is cut
/// and suffixed with a hash of the whole path, as the CLI does.
pub fn claude_slug(path: &str) -> String {
    let slug: Vec<u16> = path
        .encode_utf16()
        .map(|u| match u8::try_from(u) {
            Ok(b) if b.is_ascii_alphanumeric() => u,
            _ => u16::from(b'-'),
        })
        .collect();
    let mut out = String::from_utf16_lossy(&slug[..slug.len().min(200)]);
    if slug.len() > 200 {
        let mut n = js_hash(path).unsigned_abs();
        let mut digits = Vec::new();
        while n > 0 {
            digits.push(char::from_digit(n % 36, 36).unwrap_or('0'));
            n /= 36;
        }
        if digits.is_empty() {
            digits.push('0');
        }
        out.push('-');
        out.extend(digits.iter().rev());
    }
    out
}

/// The Java-style string hash over UTF-16 code units, wrapping to `i32`.
fn js_hash(path: &str) -> i32 {
    path.encode_utf16().fold(0i32, |h, u| {
        h.wrapping_shl(5).wrapping_sub(h).wrapping_add(i32::from(u))
    })
}

/// Claude Code's memory folder for the project at `root` (it may not exist):
/// `<config>/projects/<slug>/memory`, keyed on the main checkout of the repo
/// (so a worktree or a subdirectory shares it), else on `root` itself.
/// `None` when neither `CLAUDE_CONFIG_DIR` nor a home dir is set.
pub fn claude_memory_dir(root: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    let config = match var("CLAUDE_CONFIG_DIR") {
        Some(c) => PathBuf::from(c),
        None => Path::new(&var("HOME").or_else(|| var("USERPROFILE"))?).join(".claude"),
    };
    let key = git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    // git < 2.31 has no `--path-format`; its answer may be relative to `root`.
    .or_else(|| git(root, &["rev-parse", "--git-common-dir"]))
    .map(|c| root.join(c))
    .filter(|c| c.file_name().is_some_and(|n| n == ".git"))
    .and_then(|c| c.parent().map(Path::to_path_buf))
    .unwrap_or_else(|| root.to_path_buf());
    let key = key
        .canonicalize()
        .or_else(|_| std::path::absolute(&key))
        .unwrap_or(key);
    Some(
        config
            .join("projects")
            .join(claude_slug(&display_path(&key)))
            .join("memory"),
    )
}

/// The memory folders agents keep for the project at `root`, in a fixed
/// order, each resolved as [`resolve`] does; they may not exist yet.
pub fn agent_dirs(root: &Path) -> Vec<PathBuf> {
    [claude_memory_dir(root, &|k| std::env::var(k).ok())]
        .into_iter()
        .flatten()
        .map(|d| resolve(&d).unwrap_or(d))
        .collect()
}

/// `p` as an absolute display path with `/` separators on every OS; a Windows
/// verbatim prefix (`\\?\`, `\\?\UNC\`) is dropped so page paths stay plain.
pub fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if let Some(unc) = s.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else {
        s.strip_prefix("//?/").map_or(s.clone(), str::to_string)
    }
}

/// An external memory folder as one stable absolute path: canonical when it
/// exists (no `..`, no symlink aliases, so it matches watch events), else its
/// canonical parent plus its name, else merely absolute.
pub fn resolve(dir: &Path) -> std::io::Result<PathBuf> {
    dir.canonicalize().or_else(|_| {
        let abs = std::path::absolute(dir)?;
        Ok(
            match (abs.parent().map(Path::canonicalize), abs.file_name()) {
                (Some(Ok(p)), Some(name)) => p.join(name),
                _ => abs,
            },
        )
    })
}

/// The origin of each file git tracks under `dir`, keyed by its path relative
/// to `dir` with `/`. A file missing from the map (untracked, only staged, no
/// commit, no repo, no git) is `Origin::Local`. A note is `mine` when its
/// author's email is `git config user.email`, ignoring case (none: no note
/// is); the email is only compared, never kept. Four git calls per folder.
pub fn repo_origins(dir: &Path) -> BTreeMap<String, Origin> {
    let names = |s: Option<String>| -> Vec<String> {
        s.unwrap_or_default()
            .split('\0')
            .filter(|n| !n.is_empty())
            .map(String::from)
            .collect()
    };
    let Some(tracked) = git(
        dir,
        &["ls-tree", "-z", "-r", "--name-only", "HEAD", "--", "."],
    ) else {
        return BTreeMap::new();
    };
    let edited = names(git(
        dir,
        &["diff", "-z", "--relative", "--name-only", "HEAD", "--", "."],
    ));
    // `-z` output: `\0<email>\t<author>\0\n<name>\0<name>\0` per commit,
    // newest first; an email holds no tab.
    let log = git(
        dir,
        &[
            "-c",
            "log.showSignature=false",
            "log",
            "-z",
            "--relative",
            "--format=%x00%ae%x09%an",
            "--name-only",
            "--",
            ".",
        ],
    )
    .unwrap_or_default();
    let me = git(dir, &["config", "user.email"])
        .filter(|e| !e.is_empty())
        .map(|e| e.to_lowercase());
    // Per file: its last commit's author name and whether that author is me.
    let mut authors: BTreeMap<&str, (&str, bool)> = BTreeMap::new();
    let (mut author, mut after_gap) = (None, false);
    for t in log.split('\0') {
        if t.is_empty() {
            after_gap = true;
        } else if after_gap {
            let (email, name) = t.split_once('\t').unwrap_or(("", t));
            let mine = !email.is_empty() && me.as_deref() == Some(&email.to_lowercase());
            (author, after_gap) = (Some((name, mine)), false);
        } else if let Some(a) = author {
            authors
                .entry(t.strip_prefix('\n').unwrap_or(t))
                .or_insert(a);
        }
    }
    names(Some(tracked))
        .into_iter()
        .map(|n| {
            let (author, mine) = authors
                .get(n.as_str())
                .map_or((None, false), |(a, m)| (Some((*a).to_string()), *m));
            let origin = Origin::Repo {
                author,
                edited: edited.contains(&n),
                mine,
            };
            (n, origin)
        })
        .collect()
}

pub fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests;
