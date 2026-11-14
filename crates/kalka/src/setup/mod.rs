//! `kalka setup`: writes MCP config, skills and hooks into a project, and undoes exactly what it wrote.

pub mod edit;

use crate::{compile, memory, CompileOptions, WikiError};
use clap::ValueEnum;
use edit::Outcome;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const SKILL_TEMPLATE: &str = include_str!("SKILL.md");
const SKILL_DIR: &str = "skills/kalka-navigation";
const PI_HOOK: &str = include_str!("kalka-hint.ts");
const PI_HOOK_FILE: &str = ".pi/extensions/kalka-hint/index.ts";
/// The command the agents run.
const CMD: &str = "kalka";
/// The navigation steps for an agent that reads `AGENTS.md` (Codex, Cursor), between markers so
/// `--remove` takes out exactly them.
const NAV_RULES: &str = include_str!("navigation.md");
const NAV_BEGIN: &str = "<!-- kalka:navigation -->";
const NAV_END: &str = "<!-- /kalka:navigation -->";

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Pi,
    Cursor,
    /// Its MCP config is global (`~/.codex/config.toml`), so it is never detected.
    Codex,
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

/// Runs the agent CLI `name` with `args` in `root`; the error is a short reason for the user.
/// `mcp get` may fail: its status says whether the server exists.
fn cli(name: &str, root: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let exe = find_on_path(name).ok_or(format!("{name} is not on PATH"))?;
    let o = Command::new(exe)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run {name}: {e}"))?;
    if o.status.success() || args[1] == "get" {
        return Ok((
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
        ));
    }
    let err = String::from_utf8_lossy(&o.stderr);
    Err(format!(
        "`{name} {}` failed: {}",
        args[..2].join(" "),
        err.trim().lines().next().unwrap_or("no message")
    ))
}

fn claude(root: &Path, args: &[&str]) -> Result<(bool, String), String> {
    cli("claude", root, args)
}

/// Codex's MCP server, through its CLI. Its config is global, so one entry serves every project:
/// Codex starts the server in the directory it runs in.
fn codex_mcp(root: &Path, remove: bool) -> Outcome {
    let run = || -> Result<Outcome, String> {
        let (found, text) = cli("codex", root, &["mcp", "get", "kalka"])?;
        if found {
            let has = |l: &str| text.lines().any(|t| t.trim() == l);
            if !(has(&format!("command: {CMD}")) && has("args: serve --source .")) {
                return Ok(Outcome::Foreign(
                    "another \"kalka\" server is configured".into(),
                ));
            }
            if !remove {
                return Ok(Outcome::AlreadyThere);
            }
            cli("codex", root, &["mcp", "remove", "kalka"])?;
            return Ok(Outcome::Done);
        }
        if remove {
            return Ok(Outcome::AlreadyThere);
        }
        cli(
            "codex",
            root,
            &["mcp", "add", "kalka", "--", CMD, "serve", "--source", "."],
        )?;
        Ok(Outcome::Done)
    };
    run().unwrap_or_else(Outcome::Foreign)
}

/// Claude Code's own local-scope MCP config, through its CLI.
fn claude_local_mcp(root: &Path, entry: &Value, remove: bool) -> Outcome {
    let run = || -> Result<Outcome, String> {
        let (found, text) = claude(root, &["mcp", "get", "kalka"])?;
        if found {
            let has = |l: &str| text.lines().any(|t| t.trim() == l);
            let ours = text.lines().any(|t| t.trim().starts_with("Scope: Local"))
                && has(&format!("Command: {CMD}"))
                && has("Args: serve --source .");
            if !ours {
                return Ok(Outcome::Foreign(
                    "another \"kalka\" server is configured".into(),
                ));
            }
            if !remove {
                return Ok(Outcome::AlreadyThere);
            }
            claude(root, &["mcp", "remove", "kalka", "--scope", "local"])?;
            return Ok(Outcome::Done);
        }
        if remove {
            return Ok(Outcome::AlreadyThere);
        }
        let json = entry.to_string();
        claude(
            root,
            &["mcp", "add-json", "kalka", &json, "--scope", "local"],
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
        // Only the project folder: Cursor on PATH says nothing about this project.
        if root.join(".cursor").is_dir() {
            a.push(Agent::Cursor);
        }
        if a.is_empty() {
            return Err(WikiError::Setup(
                "no agent found: pass --agent claude,pi,cursor,codex".into(),
            ));
        }
        a
    };

    if let Some(top) = git(root, &["rev-parse", "--show-toplevel"]) {
        if std::fs::canonicalize(top).ok() != root.canonicalize().ok() {
            r.line("warning: this is not the git toplevel; agents must be started from here")?;
        }
    }

    let skill_text = SKILL_TEMPLATE.replace("<memory-dir>", memory::REPO_DIR);
    let skill_text = skill_text.as_str();
    if !opts.remove {
        let d = root.join(memory::REPO_DIR);
        let what = format!("memory folder {}", memory::display_path(&d));
        let existed = d.is_dir();
        std::fs::create_dir_all(&d)?;
        let o = if existed {
            Outcome::AlreadyThere
        } else {
            Outcome::Done
        };
        r.item(&what, false, Ok(o))?;
        let agent_dirs = memory::agent_dirs(root);
        for d in &agent_dirs {
            let state = if d.is_dir() {
                "found"
            } else {
                "not yet created"
            };
            r.line(&format!(
                "agent memory folder {} ({state})",
                memory::display_path(d)
            ))?;
        }
        if agent_dirs.is_empty() {
            r.line("no agent memory folders found")?;
        }
        let wiki = root.join(".wiki");
        let c = CompileOptions {
            incremental: true,
            memory: agent_dirs,
            ..CompileOptions::default()
        };
        let res = crate::dirlock::exclusive(&wiki, || compile(root, &wiki, &c))?;
        r.line(&format!("Compiled {} pages into .wiki", res.pages_total))?;
        if res.pages_total == 0 {
            r.line("warning: no pages compiled; is this the project root?")?;
        }
    }

    if !opts.remove && find_on_path("kalka").is_none() {
        r.line("warning: kalka is not on PATH; the agents will not find it until it is")?;
    }
    let entry = json!({"command": CMD, "args": ["serve", "--source", "."]});
    // Local scope only: repo scope puts just `.wiki/` in `.gitignore`.
    let mut ignore = vec![".wiki/".to_string(), format!("/{}/", memory::REPO_DIR)];
    let mut cursor_added = false;

    for agent in agents {
        let (name, pi) = match agent {
            Agent::Claude => ("claude", false),
            Agent::Pi => ("pi", true),
            Agent::Cursor => {
                let file = ".cursor/mcp.json";
                let path = root.join(file);
                let res = || {
                    if opts.remove {
                        edit::remove_entry(root, &path, &["mcpServers"], "kalka", &entry)
                    } else {
                        edit::upsert_entry(&path, &["mcpServers"], "kalka", &entry)
                    }
                };
                let before = path.exists();
                if r.file_item(root, file, &format!("{file} kalka server"), false, res)? {
                    cursor_added |= !opts.remove;
                    if !before {
                        ignore.push(format!("/{file}"));
                    }
                }
                continue;
            }
            Agent::Codex => {
                let o = codex_mcp(root, opts.remove);
                r.item("Codex kalka server (~/.codex/config.toml)", false, Ok(o))?;
                // The navigation steps, which Codex otherwise never reads.
                let file = "AGENTS.md";
                let path = root.join(file);
                let block = format!("{NAV_BEGIN}\n{NAV_RULES}{NAV_END}\n");
                let res = || {
                    if opts.remove {
                        edit::remove_block(root, &path, &block)
                    } else {
                        edit::upsert_block(&path, NAV_BEGIN, &block)
                    }
                };
                let what = "AGENTS.md kalka navigation steps";
                // pi reads AGENTS.md instead of CLAUDE.md when both exist, so
                // beside a CLAUDE.md the steps get no AGENTS.md of their own; one
                // an earlier setup created holds only them and goes.
                let lone = std::fs::read_to_string(&path).is_ok_and(|t| t == block);
                if !opts.remove && root.join("CLAUDE.md").exists() && (lone || !path.exists()) {
                    if lone {
                        edit::remove_block(root, &path, &block)?;
                        r.line("removed AGENTS.md: it held only the kalka steps and hid CLAUDE.md from pi")?;
                    }
                    r.skip(
                        what,
                        "an AGENTS.md beside CLAUDE.md hides CLAUDE.md from pi; rename CLAUDE.md \
                         to AGENTS.md, put `@AGENTS.md` in a new CLAUDE.md for Claude Code, and \
                         run setup again",
                    )?;
                } else {
                    let before = path.exists();
                    if r.file_item(root, file, what, false, res)? && !before {
                        ignore.push(format!("/{file}"));
                    }
                }
                continue;
            }
        };
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
                    edit::remove_entry(root, &path, &["mcpServers"], "kalka", &value)
                } else {
                    edit::upsert_entry(&path, &["mcpServers"], "kalka", &value)
                }
            };
            if r.file_item(root, file, &format!("{file} kalka server"), pi, res)? && pi {
                ignore.push("/.pi/mcp.json".into());
            }
        } else {
            let o = claude_local_mcp(root, &entry, opts.remove);
            r.item("Claude Code local kalka server", false, Ok(o))?;
        }
        // Skill: pi only. Claude Code shows the server instructions, which carry the navigation
        // steps (a navigation skill cost a turn per task there), and keeps its own memory.
        if pi {
            let skill = Path::new(".agents").join(SKILL_DIR);
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
                    ignore.push("/.pi/extensions/kalka-hint/".into());
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
    if cursor_added {
        r.line("Cursor loads the server only after you approve it: in its MCP settings, or `cursor-agent mcp enable kalka`.")?;
    }
    Ok(r.ok)
}
