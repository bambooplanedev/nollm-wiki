//! `wiki setup`: writes MCP config, skills and hooks into a project, and undoes exactly what it wrote.

pub mod edit;

use crate::{compile, memory, CompileOptions, WikiError};
use clap::ValueEnum;
use edit::Outcome;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const SKILL_TEMPLATE: &str = include_str!("SKILL.md");
const SKILL_DIR: &str = "skills/wiki-navigation";
const PI_HOOK: &str = include_str!("wiki-hint.ts");
const PI_HOOK_FILE: &str = ".pi/extensions/wiki-hint/index.ts";
/// The command the agents run.
const CMD: &str = "wiki";

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Pi,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    /// Committed files, for the whole team.
    Repo,
    /// This checkout only, never committed.
    Local,
}

pub struct SetupOptions {
    /// `None`: detect.
    pub agents: Option<Vec<Agent>>,
    pub scope: Scope,
    pub hook: bool,
    pub remove: bool,
}

#[cfg(unix)]
fn executable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &std::fs::Metadata) -> bool {
    true
}

/// First executable regular file called `name` on `PATH`.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.metadata().is_ok_and(|m| m.is_file() && executable(&m)))
}

#[allow(clippy::struct_excessive_bools)]
struct Report<'a> {
    out: &'a mut dyn Write,
    remove: bool,
    /// Nothing skipped so far.
    ok: bool,
    wrote_pi: bool,
    /// Local-scope install: skip files git already tracks.
    guard_tracked: bool,
}

/// `memory::git`, run only when git is on `PATH`.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    find_on_path("git").and_then(|_| memory::git(root, args))
}

/// Whether git (when on `PATH`) tracks `file` in `root`.
fn tracked(root: &Path, file: &str) -> bool {
    git(root, &["ls-files", "--error-unmatch", "--", file]).is_some()
}

/// The repository's `info/exclude`: asked of git when it is on `PATH`, else `.git/info/exclude`.
fn exclude_file(root: &Path) -> Option<PathBuf> {
    git(root, &["rev-parse", "--git-path", "info/exclude"])
        .map(|p| root.join(p))
        .or_else(|| {
            root.join(".git")
                .is_dir()
                .then(|| root.join(".git/info/exclude"))
        })
}

/// `root`'s path below the worktree top, with a trailing `/` (empty at the top or without git).
/// Patterns in `info/exclude` anchor to the top, so a subdirectory's lines need it.
fn worktree_prefix(root: &Path) -> String {
    git(root, &["rev-parse", "--show-prefix"]).unwrap_or_default()
}

impl Report<'_> {
    fn line(&mut self, s: &str) -> Result<(), WikiError> {
        writeln!(self.out, "{s}")?;
        Ok(())
    }

    fn skip(&mut self, what: &str, why: &str) -> Result<(), WikiError> {
        self.ok = false;
        self.line(&format!("skipped {what}: {why}"))
    }

    /// `item` for a file in `root`, except that a local-scope install skips a file git tracks:
    /// an exclude line does nothing for it, so the "local" change would become a diff.
    fn file_item(
        &mut self,
        root: &Path,
        file: &str,
        what: &str,
        pi: bool,
        res: impl FnOnce() -> std::io::Result<Outcome>,
    ) -> Result<bool, WikiError> {
        if self.guard_tracked && tracked(root, file) {
            self.skip(what, "tracked by git; use --scope repo")?;
            return Ok(false);
        }
        self.item(what, pi, res())
    }

    /// Reports one outcome; `true` when the item is in place (written or already there).
    fn item(
        &mut self,
        what: &str,
        pi: bool,
        r: std::io::Result<Outcome>,
    ) -> Result<bool, WikiError> {
        let r = r?;
        let present = !matches!(r, Outcome::Foreign(_));
        match (r, self.remove) {
            (Outcome::Done, false) => {
                self.wrote_pi |= pi;
                self.line(&format!("wrote {what}"))
            }
            (Outcome::Done, true) => self.line(&format!("removed {what}")),
            (Outcome::AlreadyThere, false) => self.line(&format!("already set up: {what}")),
            (Outcome::AlreadyThere, true) => self.line(&format!("not present: {what}")),
            (Outcome::Foreign(why), _) => self.skip(what, &why),
        }?;
        Ok(present)
    }
}

/// Runs `claude` with `args` in `root`; the error is a short reason for the user.
fn claude(root: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let exe = find_on_path("claude").ok_or("claude is not on PATH")?;
    let o = Command::new(exe)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run claude: {e}"))?;
    if o.status.success() || args[1] == "get" {
        return Ok((
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
        ));
    }
    let err = String::from_utf8_lossy(&o.stderr);
    Err(format!(
        "`claude {}` failed: {}",
        args[..2].join(" "),
        err.trim().lines().next().unwrap_or("no message")
    ))
}

/// Claude Code's own local-scope MCP config, through its CLI.
fn claude_local_mcp(root: &Path, entry: &Value, remove: bool) -> Outcome {
    let run = || -> Result<Outcome, String> {
        let (found, text) = claude(root, &["mcp", "get", "wiki"])?;
        if found {
            let has = |l: &str| text.lines().any(|t| t.trim() == l);
            let ours = text.lines().any(|t| t.trim().starts_with("Scope: Local"))
                && has(&format!("Command: {CMD}"))
                && has("Args: serve --source .");
            if !ours {
                return Ok(Outcome::Foreign(
                    "another \"wiki\" server is configured".into(),
                ));
            }
            if !remove {
                return Ok(Outcome::AlreadyThere);
            }
            claude(root, &["mcp", "remove", "wiki", "--scope", "local"])?;
            return Ok(Outcome::Done);
        }
        if remove {
            return Ok(Outcome::AlreadyThere);
        }
        let json = entry.to_string();
        claude(
            root,
            &["mcp", "add-json", "wiki", &json, "--scope", "local"],
        )?;
        Ok(Outcome::Done)
    };
    run().unwrap_or_else(Outcome::Foreign)
}

/// Installs (or with `remove`, undoes) what `opts` asks for in `root`.
/// `Ok(false)`: something was skipped.
pub fn run(root: &Path, opts: &SetupOptions, out: &mut dyn Write) -> Result<bool, WikiError> {
    let local = opts.scope == Scope::Local;
    let mut r = Report {
        out,
        remove: opts.remove,
        ok: true,
        wrote_pi: false,
        guard_tracked: local && !opts.remove,
    };
    let agents = if let Some(a) = &opts.agents {
        a.clone()
    } else {
        let mut a = Vec::new();
        if root.join(".claude").exists()
            || root.join("CLAUDE.md").exists()
            || find_on_path("claude").is_some()
        {
            a.push(Agent::Claude);
        }
        if root.join(".pi").exists() || find_on_path("pi").is_some() {
            a.push(Agent::Pi);
        }
        if a.is_empty() {
            return Err(WikiError::Setup(
                "no agent found: pass --agent claude,pi".into(),
            ));
        }
        a
    };

    if let Some(top) = git(root, &["rev-parse", "--show-toplevel"]) {
        if std::fs::canonicalize(top).ok() != root.canonicalize().ok() {
            r.line("warning: this is not the git toplevel; agents must be started from here")?;
        }
    }

    let local_memory = memory::local_dir(root);
    // The skill's Memory section names the scope's folder; local scope without a local folder
    // gets no section (naming the committable repo folder would defeat the scope).
    let skill_text = match (&local_memory, local) {
        (Some(d), true) => SKILL_TEMPLATE.replace("<memory-dir>", &memory::display_path(d)),
        (None, true) => SKILL_TEMPLATE
            .split_once("\n# Memory\n")
            .map_or(SKILL_TEMPLATE, |(head, _)| head)
            .to_string(),
        _ => SKILL_TEMPLATE.replace("<memory-dir>", memory::REPO_DIR),
    };
    let skill_text = skill_text.as_str();
    if !opts.remove {
        let folder = match (&local_memory, local) {
            (Some(d), true) => Some(d.clone()),
            (None, true) => {
                let why = if find_on_path("git").is_none() {
                    "git is not on PATH"
                } else if memory::data_dir().is_none() {
                    "no data dir (set HOME, XDG_DATA_HOME or LOCALAPPDATA)"
                } else if memory::is_git_repo(root) {
                    "no commits yet"
                } else {
                    "not a git repo"
                };
                r.line(&format!("no local memory: {why}; no memory folder created"))?;
                None
            }
            _ => Some(root.join(memory::REPO_DIR)),
        };
        if let Some(d) = folder {
            let what = format!("memory folder {}", memory::display_path(&d));
            let existed = d.is_dir();
            std::fs::create_dir_all(&d)?;
            let o = if existed {
                Outcome::AlreadyThere
            } else {
                Outcome::Done
            };
            r.item(&what, false, Ok(o))?;
        }
        let wiki = root.join(".wiki");
        let c = CompileOptions {
            incremental: true,
            // Repo-scope notes are found by the compile itself; the local folder is external.
            memory: local_memory
                .as_ref()
                .filter(|d| local || d.is_dir())
                .and_then(|d| memory::resolve(d).ok()),
            ..CompileOptions::default()
        };
        let res = compile(root, &wiki, &c)?;
        r.line(&format!("Compiled {} pages into .wiki", res.pages_total))?;
        if res.pages_total == 0 {
            r.line("warning: no pages compiled; is this the project root?")?;
        }
    }

    if !opts.remove && find_on_path("wiki").is_none() {
        r.line("warning: wiki is not on PATH; the agents will not find it until it is")?;
    }
    let entry = json!({"command": CMD, "args": ["serve", "--source", "."]});
    let mut ignore = vec![".wiki/".to_string()];

    for agent in agents {
        let (name, pi) = match agent {
            Agent::Claude => ("claude", false),
            Agent::Pi => ("pi", true),
        };
        let skill = Path::new(if pi { ".agents" } else { ".claude" }).join(SKILL_DIR);
        // MCP
        if pi || !local {
            let (file, value) = if pi {
                let mut v = entry.clone();
                v["exposure"] = json!("direct");
                (".pi/mcp.json", v)
            } else {
                (".mcp.json", entry.clone())
            };
            let path = root.join(file);
            let res = || {
                if opts.remove {
                    edit::remove_entry(root, &path, &["mcpServers"], "wiki", &value)
                } else {
                    edit::upsert_entry(&path, &["mcpServers"], "wiki", &value)
                }
            };
            if r.file_item(root, file, &format!("{file} wiki server"), pi, res)? && pi {
                ignore.push("/.pi/mcp.json".into());
            }
        } else {
            let o = claude_local_mcp(root, &entry, opts.remove);
            r.item("Claude Code local wiki server", false, Ok(o))?;
        }
        // Skill
        let file = skill.join("SKILL.md");
        let res = || {
            if opts.remove {
                edit::remove_file(root, &root.join(&file), skill_text)
            } else {
                edit::write_file(&root.join(&file), skill_text)
            }
        };
        let what = format!("{} skill for {name}", file.display());
        if r.file_item(root, &file.to_string_lossy(), &what, pi, res)? {
            ignore.push(format!("/{}/", skill.display()));
        }
        // Hook (`--remove` always looks, so it undoes a hook installed earlier)
        if opts.hook || opts.remove {
            if pi {
                let content = PI_HOOK.replace("<cmd>", CMD);
                let path = root.join(PI_HOOK_FILE);
                let res = || {
                    if opts.remove {
                        edit::remove_file(root, &path, &content)
                    } else {
                        edit::write_file(&path, &content)
                    }
                };
                let what = format!("{PI_HOOK_FILE} hook for pi");
                if r.file_item(root, PI_HOOK_FILE, &what, true, res)? {
                    ignore.push("/.pi/extensions/wiki-hint/".into());
                }
            } else {
                let file = if local {
                    ".claude/settings.local.json"
                } else {
                    ".claude/settings.json"
                };
                let item = json!({"hooks": [{"type": "command", "command": format!("{CMD} hint --claude-hook")}]});
                let path = root.join(file);
                let parents = ["hooks", "UserPromptSubmit"];
                let res = || {
                    if opts.remove {
                        edit::remove_item(root, &path, &parents, &item)
                    } else {
                        edit::append_item(&path, &parents, &item)
                    }
                };
                if r.file_item(root, file, &format!("{file} prompt hook"), false, res)? && local {
                    ignore.push(format!("/{file}"));
                }
            }
        }
    }

    if opts.remove {
        r.line("kept the ignore lines and .wiki/ (delete them by hand if unwanted)")?;
    } else if !local {
        r.item(
            ".wiki/ in .gitignore",
            false,
            edit::ensure_line(&root.join(".gitignore"), ".wiki/"),
        )?;
    } else if let Some(exclude) = exclude_file(root) {
        let prefix = worktree_prefix(root);
        for line in ignore {
            let line = match (prefix.is_empty(), line.strip_prefix('/')) {
                (true, _) => line,
                (false, Some(rest)) => format!("/{prefix}{rest}"),
                (false, None) => format!("/{prefix}{line}"),
            };
            r.item(
                &format!("{line} in .git/info/exclude"),
                false,
                edit::ensure_line(&exclude, &line),
            )?;
        }
    } else {
        r.line("not a git repository: nothing to exclude")?;
    }
    if r.wrote_pi {
        r.line("pi loads project files only after you trust the project (/trust, or --approve in print mode).")?;
    }
    Ok(r.ok)
}
