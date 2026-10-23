//! `wiki setup`: writes MCP config, skills and hooks into a project, and undoes exactly what it wrote.

pub mod edit;

use crate::{compile, CompileOptions, WikiError};
use clap::ValueEnum;
use edit::Outcome;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const SKILL: &str = include_str!("SKILL.md");
const SKILL_DIR: &str = "skills/wiki-navigation";

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

struct Report<'a> {
    out: &'a mut dyn Write,
    remove: bool,
    /// Nothing skipped so far.
    ok: bool,
    wrote_pi: bool,
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
fn claude_local_mcp(root: &Path, cmd: &str, entry: &Value, remove: bool) -> Outcome {
    let run = || -> Result<Outcome, String> {
        let (found, text) = claude(root, &["mcp", "get", "wiki"])?;
        if found {
            let has = |l: &str| text.lines().any(|t| t.trim() == l);
            let ours = text.lines().any(|t| t.trim().starts_with("Scope: Local"))
                && has(&format!("Command: {cmd}"))
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

    if find_on_path("git").is_some() {
        if let Ok(o) = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(root)
            .output()
        {
            let top = String::from_utf8_lossy(&o.stdout);
            if o.status.success()
                && std::fs::canonicalize(top.trim()).ok() != root.canonicalize().ok()
            {
                r.line("warning: this is not the git toplevel; agents must be started from here")?;
            }
        }
    }

    if !opts.remove {
        let wiki = root.join(".wiki");
        let c = CompileOptions {
            incremental: true,
            ..CompileOptions::default()
        };
        let res = compile(root, &wiki, &c)?;
        r.line(&format!("Compiled {} pages into .wiki", res.pages_total))?;
        if res.pages_total == 0 {
            r.line("warning: no pages compiled; is this the project root?")?;
        }
    }

    let on_path = find_on_path("wiki").is_some();
    let cmd = if on_path || !local {
        "wiki".to_string()
    } else {
        std::env::current_exe()?.to_string_lossy().into_owned()
    };
    if !on_path && !opts.remove && !local {
        r.line("warning: wiki is not on PATH; the agents will not find it until it is")?;
    }
    let entry = json!({"command": cmd, "args": ["serve", "--source", "."]});
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
            let res = if opts.remove {
                edit::remove_entry(root, &path, &["mcpServers"], "wiki", &value)
            } else {
                edit::upsert_entry(&path, &["mcpServers"], "wiki", &value)
            };
            if r.item(&format!("{file} wiki server"), pi, res)? && pi {
                ignore.push("/.pi/mcp.json".into());
            }
        } else {
            let o = claude_local_mcp(root, &cmd, &entry, opts.remove);
            r.item("Claude Code local wiki server", false, Ok(o))?;
        }
        // Skill
        let file = skill.join("SKILL.md");
        let res = if opts.remove {
            edit::remove_file(root, &root.join(&file), SKILL)
        } else {
            edit::write_file(&root.join(&file), SKILL)
        };
        if r.item(&format!("{} skill for {name}", file.display()), pi, res)? {
            ignore.push(format!("/{}/", skill.display()));
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
    } else if root.join(".git").is_dir() {
        for line in ignore {
            r.item(
                &format!("{line} in .git/info/exclude"),
                false,
                edit::ensure_line(&root.join(".git/info/exclude"), &line),
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
