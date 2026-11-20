//! `wiki setup`: writes MCP config, skills and hooks into a project, and undoes exactly what it wrote.

mod codex;
pub mod edit;

use crate::{compile, memory, CompileOptions, WikiError};
use clap::ValueEnum;
use edit::Outcome;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use wiki_code::OUT_DIR;

const SKILL_TEMPLATE: &str = include_str!("SKILL.md");
const SKILL_DIR: &str = "skills/wiki-navigation";
const PI_HOOK: &str = include_str!("wiki-hint.ts");
const PI_HOOK_FILE: &str = ".pi/extensions/wiki-hint/index.ts";
/// The command the agents run, found on `PATH`.
const CMD: &str = "wiki";
/// The command the agents run when this wiki is the nollm-wiki home's: found without `PATH`, and
/// with no one's home path in a file the team shares. Run by a shell.
const LAUNCH: &str = "\"${NOLLM_WIKI_HOME:-$HOME/.nollm-wiki-home}/bin/wiki\"";
/// [`LAUNCH`] for pi, which starts wiki without a shell.
const PI_LAUNCH: &str = r#"path.join(process.env.NOLLM_WIKI_HOME || path.join(os.homedir(), ".nollm-wiki-home"), "bin/wiki")"#;
/// The navigation steps for an agent that reads `AGENTS.md` (Codex, Cursor), between markers so
/// `--remove` takes out exactly them.
const NAV_RULES: &str = include_str!("navigation.md");
const NAV_BEGIN: &str = "<!-- wiki:navigation -->";
const NAV_END: &str = "<!-- /wiki:navigation -->";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Pi,
    Cursor,
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
    /// Empty: no agent, only what is not agent-specific.
    pub agents: Vec<Agent>,
    pub scope: Scope,
    /// The prompt hook (`wiki hint` on each prompt) for Claude Code and pi. Off, a run
    /// removes one an earlier setup wrote.
    pub hook: bool,
    pub remove: bool,
    /// Agent memory: the `.agents/memory` folder, the skill's Memory section and the server's
    /// memory. Off, the server runs with `--memory none`. `--remove` takes either variant.
    pub memory: bool,
}

/// The MCP entry that starts the server with `cmd` ([`CMD`] or, through `sh`, [`LAUNCH`]);
/// without memory, `serve --memory none`.
fn entry(cmd: &str, memory: bool) -> Value {
    let mut a = vec!["serve", "--source", "."];
    if !memory {
        a.extend(["--memory", "none"]);
    }
    if cmd == CMD {
        return json!({"command": CMD, "args": a});
    }
    json!({"command": "sh", "args": ["-c", format!("exec {cmd} {}", a.join(" "))]})
}

/// pi's extension starting `cmd`; `old`: as setup wrote it while the hook was on by default.
fn pi_hook(cmd: &str, old: bool) -> String {
    let mut text = PI_HOOK.replace("<out>", OUT_DIR);
    if old {
        text = text.replace("setup --hook)", "setup unless --no-hook)");
    }
    if cmd == CMD {
        return text.replace("<cmd>", CMD);
    }
    text.replace("import path", "import os from \"node:os\";\nimport path")
        .replace("\"<cmd>\"", PI_LAUNCH)
}

/// When this executable is the nollm-wiki home's `bin/wiki` (the home is `NOLLM_WIKI_HOME`, else
/// `$HOME/.nollm-wiki-home`): whether that home is `$HOME/.nollm-wiki-home`, where [`LAUNCH`] looks
/// without `NOLLM_WIKI_HOME`.
fn nollm_wiki_home() -> Option<bool> {
    let var = |k| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let default = var("HOME").map(|h| h.join(".nollm-wiki-home"));
    let home = var("NOLLM_WIKI_HOME").or_else(|| default.clone())?;
    let real = |p: &Path| p.canonicalize().ok();
    let exe = std::env::current_exe().ok()?;
    (real(&exe)? == real(&home.join("bin/wiki"))?)
        .then(|| default.and_then(|d| real(&d)) == real(&home))
}

/// The skill, with its Memory section only when `memory`.
fn skill_text(memory: bool) -> String {
    let text = SKILL_TEMPLATE.replace("<memory-dir>", memory::REPO_DIR);
    match text.split_once("\n# Memory\n") {
        Some((nav, _)) if !memory => format!("{}\n", nav.trim_end()),
        _ => text,
    }
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
    wrote: usize,
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
        res: impl FnOnce() -> std::io::Result<Outcome>,
    ) -> Result<bool, WikiError> {
        if self.guard_tracked && tracked(root, file) {
            self.skip(what, "tracked by git; use --scope repo")?;
            return Ok(false);
        }
        let wrote = self.wrote;
        let present = self.item(what, res())?;
        // Repo scope: a file git ignores never reaches the teammates it was written for.
        if !self.guard_tracked
            && self.wrote > wrote
            && git(root, &["check-ignore", "-q", "--", file]).is_some()
        {
            self.line(&format!(
                "warning: {file} is ignored by git; teammates will not get it"
            ))?;
        }
        Ok(present)
    }

    /// Reports one outcome; `true` when the item is in place (written or already there).
    fn item(&mut self, what: &str, r: std::io::Result<Outcome>) -> Result<bool, WikiError> {
        let r = r?;
        let present = !matches!(r, Outcome::Foreign(_));
        match (r, self.remove) {
            (Outcome::Done, false) => {
                self.wrote += 1;
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
/// `mcp get` may fail: its status says whether the server exists, and its text is stdout. Any
/// other call's text is stdout then stderr, whichever stream a CLI prints a note on.
fn cli(name: &str, root: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let exe = find_on_path(name).ok_or(format!("{name} is not on PATH"))?;
    // Found through a relative `PATH` entry, it is resolved here: `root` may be another folder.
    let exe = std::path::absolute(exe).map_err(|e| format!("could not run {name}: {e}"))?;
    let o = Command::new(exe)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run {name}: {e}"))?;
    let get = args[1] == "get";
    if o.status.success() || get {
        let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
        if !get {
            text.push_str(&String::from_utf8_lossy(&o.stderr));
        }
        return Ok((o.status.success(), text));
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

/// A switch to the other memory variant leaves alone an entry with settings setup did not write.
fn other_settings(remove: &str) -> Outcome {
    Outcome::Foreign(format!(
        "wiki's server carries other settings; remove it with `{remove}` and run `wiki setup` again"
    ))
}

/// The add after a switch's remove failed: the old entry is gone.
fn lost(e: &str) -> String {
    format!(
        "wiki's server entry was removed and the new one could not be added ({e}); \
         run `wiki setup` again to add it"
    )
}

/// Claude Code's own local-scope MCP config for the server [`CMD`], through its CLI. Each of `own`
/// is ours; another one is replaced by `own[0]` only when `mcp get` shows nothing but what
/// setup's own `add-json` yields (scope, status and its issue, type, command, args, the remove
/// hint): an `Environment:`, `Timeout:` or any other line is a setting setup did not write.
fn claude_local_mcp(root: &Path, own: &[Value], remove: bool) -> Outcome {
    // Each entry's `Command:` and `Args:` lines, the latter also as Claude Code shows it: it
    // prints `${VAR:-default}` as `${VAR}`, so [`LAUNCH`] reads `"${NOLLM_WIKI_HOME}/bin/wiki"`.
    let lines: Vec<[String; 3]> = own
        .iter()
        .map(|v| {
            let a: Vec<&str> = v["args"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            [
                format!("Command: {}", v["command"].as_str().unwrap_or_default()),
                format!("Args: {}", a.join(" ")),
                format!("Args: {}", a.join(" ")).replace(":-$HOME/.nollm-wiki-home", ""),
            ]
        })
        .collect();
    let run = || -> Result<Outcome, String> {
        let (found, text) = claude(root, &["mcp", "get", CMD])?;
        if found {
            let has = |l: &str| text.lines().any(|t| t.trim() == l);
            let shows = |[c, a, shown]: &[String; 3]| has(c) && (has(a) || has(shown));
            let ours = text.lines().any(|t| t.trim().starts_with("Scope: Local"))
                && lines.iter().any(shows);
            if !ours {
                return Ok(Outcome::Foreign(format!(
                    "another \"{CMD}\" server is configured"
                )));
            }
            if !remove && shows(&lines[0]) {
                return Ok(Outcome::AlreadyThere);
            }
            let own_line = |l: &str| {
                [
                    "Scope: ",
                    "Status: ",
                    "Issue: ",
                    "To remove this server, run: ",
                ]
                .iter()
                .any(|k| l.starts_with(k))
                    || l == format!("{CMD}:")
                    || l == "Type: stdio"
                    || lines.iter().flatten().any(|o| o == l)
            };
            let other = !text
                .lines()
                .map(str::trim)
                .all(|l| l.is_empty() || own_line(l));
            if other && !remove {
                return Ok(other_settings(&format!(
                    "claude mcp remove {CMD} --scope local"
                )));
            }
            claude(root, &["mcp", "remove", CMD, "--scope", "local"])?;
            if remove {
                return Ok(Outcome::Done);
            }
        } else if remove {
            return Ok(Outcome::AlreadyThere);
        }
        let json = own[0].to_string();
        claude(root, &["mcp", "add-json", CMD, &json, "--scope", "local"]).map_err(|e| {
            if found {
                lost(&e)
            } else {
                e
            }
        })?;
        Ok(Outcome::Done)
    };
    run().unwrap_or_else(Outcome::Foreign)
}

/// The agents found in `root`.
pub fn detect(root: &Path) -> Vec<Agent> {
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
    // Only the project folder: Cursor or Codex on PATH says nothing about this project.
    if root.join(".cursor").is_dir() {
        a.push(Agent::Cursor);
    }
    // Codex's own home (setup in the home folder) is no project's.
    if root.join(".codex").is_dir() && !codex::codex_home(root) {
        a.push(Agent::Codex);
    }
    a
}

/// An answer to the agent question: numbers (from 1, in [`Agent`] order) or names, separated
/// by commas or spaces; empty is none.
pub fn parse_agents(line: &str) -> Result<Vec<Agent>, String> {
    let all = Agent::value_variants();
    let mut chosen = Vec::new();
    for word in line
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty())
    {
        let agent = match word.parse::<usize>() {
            Ok(n) => n.checked_sub(1).and_then(|i| all.get(i)).copied(),
            Err(_) => Agent::from_str(word, true).ok(),
        }
        .ok_or_else(|| format!("no agent {word:?}: choose 1-{} or names", all.len()))?;
        if !chosen.contains(&agent) {
            chosen.push(agent);
        }
    }
    Ok(chosen)
}

/// Installs (or with `remove`, undoes) what `opts` asks for in `root`.
/// `Ok(false)`: something was skipped.
pub fn run(root: &Path, opts: &SetupOptions, out: &mut dyn Write) -> Result<bool, WikiError> {
    let local = opts.scope == Scope::Local;
    let mut r = Report {
        out,
        remove: opts.remove,
        ok: true,
        wrote: 0,
        wrote_pi: false,
        guard_tracked: local && !opts.remove,
    };

    if let Some(top) = git(root, &["rev-parse", "--show-toplevel"]) {
        if std::fs::canonicalize(top).ok() != root.canonicalize().ok() {
            r.line("warning: this is not the git toplevel; agents must be started from here")?;
        }
    }

    // Own variants, this run's choice first; setup rewrites any of them and `--remove` takes any.
    let home = nollm_wiki_home();
    let cmds: &[&str] = if home.is_some() {
        &[LAUNCH, CMD]
    } else {
        &[CMD, LAUNCH]
    };
    let choice = [opts.memory, !opts.memory];
    let skills = choice.map(skill_text);
    let skills = skills.each_ref().map(String::as_str);
    let entries: Vec<Value> = cmds
        .iter()
        .flat_map(|c| choice.map(|m| entry(c, m)))
        .collect();
    if !opts.remove {
        // As the registered server compiles: `--memory none` has no memory at all.
        let mut agent_dirs = Vec::new();
        if opts.memory {
            let d = root.join(memory::REPO_DIR);
            let what = format!("memory folder {}", memory::display_path(&d));
            let existed = d.is_dir();
            std::fs::create_dir_all(&d)?;
            let o = if existed {
                Outcome::AlreadyThere
            } else {
                Outcome::Done
            };
            r.item(&what, Ok(o))?;
            agent_dirs = memory::agent_dirs(root);
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
        }
        let wiki = root.join(OUT_DIR);
        let c = CompileOptions {
            incremental: true,
            memory: agent_dirs,
            repo_memory: opts.memory,
            ..CompileOptions::default()
        };
        let res = crate::dirlock::exclusive(&wiki, || compile(root, &wiki, &c))?;
        r.line(&format!(
            "Compiled {} pages into {OUT_DIR}",
            res.pages_total
        ))?;
        if res.pages_total == 0 {
            r.line("warning: no pages compiled; is this the project root?")?;
        }
    }

    let warning = match home {
        None if find_on_path(CMD).is_none() => {
            "warning: wiki is not on PATH; the agents will not find it until it is"
        }
        Some(false) => {
            "warning: the agents find wiki through NOLLM_WIKI_HOME; set it in their environment"
        }
        _ => "",
    };
    if !opts.remove && !opts.agents.is_empty() && !warning.is_empty() {
        r.line(warning)?;
    }
    // Local scope only: repo scope puts just `.nollm-wiki/` in `.gitignore`.
    let mut ignore = vec![format!("{OUT_DIR}/")];
    if opts.memory {
        ignore.push(format!("/{}/", memory::REPO_DIR));
    }
    let (mut cursor_added, mut codex_added) = (false, false);

    for &agent in &opts.agents {
        let wrote = r.wrote;
        let pi = match agent {
            Agent::Claude => false,
            Agent::Pi => true,
            Agent::Cursor => {
                let file = ".cursor/mcp.json";
                let path = root.join(file);
                let res = || {
                    if opts.remove {
                        edit::remove_entry(root, &path, &["mcpServers"], "wiki", &entries)
                    } else {
                        edit::upsert_entry(&path, &["mcpServers"], "wiki", &entries)
                    }
                };
                let before = path.exists();
                if r.file_item(root, file, &format!("{file} wiki server"), res)? {
                    cursor_added |= !opts.remove;
                    if !before {
                        ignore.push(format!("/{file}"));
                    }
                }
                continue;
            }
            Agent::Codex => {
                let before = root.join(codex::FILE).exists();
                if codex::run(root, &mut r, opts.remove, &entries)? {
                    codex_added |= !opts.remove;
                    if !before {
                        ignore.push(format!("/{}", codex::FILE));
                    }
                }
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
                let what = "AGENTS.md wiki navigation steps";
                // pi reads AGENTS.md instead of CLAUDE.md when both exist, so
                // beside a CLAUDE.md the steps get no AGENTS.md of their own; one
                // an earlier setup created holds only them and goes.
                let lone = std::fs::read_to_string(&path).is_ok_and(|t| t == block);
                if !opts.remove && root.join("CLAUDE.md").exists() && (lone || !path.exists()) {
                    if lone {
                        edit::remove_block(root, &path, &block)?;
                        r.line("removed AGENTS.md: it held only the wiki steps and hid CLAUDE.md from pi")?;
                    }
                    r.skip(
                        what,
                        "an AGENTS.md beside CLAUDE.md hides CLAUDE.md from pi; rename CLAUDE.md \
                         to AGENTS.md, put `@AGENTS.md` in a new CLAUDE.md for Claude Code, and \
                         run setup again",
                    )?;
                } else {
                    let before = path.exists();
                    if r.file_item(root, file, what, res)? && !before {
                        ignore.push(format!("/{file}"));
                    }
                }
                continue;
            }
        };
        // MCP
        if pi || !local {
            let (file, value) = if pi {
                let mut v = entries.clone();
                for e in &mut v {
                    e["exposure"] = json!("direct");
                }
                (".pi/mcp.json", v)
            } else {
                (".mcp.json", entries.clone())
            };
            let path = root.join(file);
            let res = || {
                if opts.remove {
                    edit::remove_entry(root, &path, &["mcpServers"], "wiki", &value)
                } else {
                    edit::upsert_entry(&path, &["mcpServers"], "wiki", &value)
                }
            };
            if r.file_item(root, file, &format!("{file} wiki server"), res)? && pi {
                ignore.push("/.pi/mcp.json".into());
            }
        } else {
            let o = claude_local_mcp(root, &entries, opts.remove);
            r.item("Claude Code local wiki server", Ok(o))?;
        }
        // Skill: pi only. Claude Code shows the server instructions, which carry the navigation
        // steps (a navigation skill cost a turn per task there), and keeps its own memory.
        if pi {
            let skill = Path::new(".agents").join(SKILL_DIR);
            let file = skill.join("SKILL.md");
            let res = || {
                if opts.remove {
                    edit::remove_file(root, &root.join(&file), &skills)
                } else {
                    edit::write_file(&root.join(&file), &skills)
                }
            };
            let what = format!("{} skill for pi", file.display());
            if r.file_item(root, &file.to_string_lossy(), &what, res)? {
                ignore.push(format!("/{}/", skill.display()));
            }
        }

        // Hook: only with `--hook`. `--remove`, and a run without `--hook`, take out one an
        // earlier setup wrote (the hook was on by default up to 0.8).
        let file = match (pi, local) {
            (true, _) => PI_HOOK_FILE,
            (false, true) => ".claude/settings.local.json",
            (false, false) => ".claude/settings.json",
        };
        let path = root.join(file);
        let pi_own: Vec<String> = [false, true]
            .iter()
            .flat_map(|&old| cmds.iter().map(move |c| pi_hook(c, old)))
            .collect();
        let pi_own: Vec<&str> = pi_own.iter().map(String::as_str).collect();
        let own: Vec<Value> = cmds
            .iter()
            .map(|c| json!({"hooks": [{"type": "command", "command": format!("{c} hint --claude-hook")}]}))
            .collect();
        let parents = ["hooks", "UserPromptSubmit"];
        let hook = |remove: bool| match (pi, remove) {
            (true, true) => edit::remove_file(root, &path, &pi_own),
            (true, false) => edit::write_file(&path, &pi_own),
            (false, true) => edit::remove_item(root, &path, &parents, &own),
            (false, false) => edit::append_item(&path, &parents, &own),
        };
        let what = if pi {
            format!("{file} hook for pi")
        } else {
            format!("{file} prompt hook")
        };
        if opts.hook || opts.remove {
            if r.file_item(root, file, &what, || hook(opts.remove))? && (pi || local) {
                ignore.push(if pi {
                    "/.pi/extensions/wiki-hint/".into()
                } else {
                    format!("/{file}")
                });
            }
        } else if !(r.guard_tracked && tracked(root, file)) {
            // A file this run would not otherwise write is left and named without failing the
            // run.
            match hook(true) {
                Ok(Outcome::Done) => r.line(&format!(
                    "removed {what}: the prompt hint is off by default now; run setup again \
                     with --hook to keep it"
                ))?,
                Ok(_) => {}
                Err(e) => {
                    let item = if pi {
                        "wiki hook for pi"
                    } else {
                        "wiki prompt hook"
                    };
                    r.line(&format!(
                        "left {file} as it is: cannot check it for {item} ({e})"
                    ))?;
                }
            }
        }
        // Relies on: inside the pi iteration only pi's own items are written.
        r.wrote_pi |= pi && r.wrote > wrote;
    }

    if opts.remove {
        r.line("kept the ignore lines and .nollm-wiki/ (delete them by hand if unwanted)")?;
    } else if !local {
        r.item(
            &format!("{OUT_DIR}/ in .gitignore"),
            edit::ensure_line(&root.join(".gitignore"), &format!("{OUT_DIR}/")),
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
        r.line("Cursor: the editor lists the server under Settings → Tools & MCP; cursor-agent loads it only after `cursor-agent mcp enable wiki`.")?;
    }
    if codex_added {
        r.line(&codex::hint(root))?;
    }
    Ok(r.ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_answer_takes_numbers_and_names() {
        use Agent::{Claude, Codex, Cursor, Pi};
        assert_eq!(parse_agents(""), Ok(vec![]));
        assert_eq!(parse_agents("  \n"), Ok(vec![]));
        assert_eq!(parse_agents("1"), Ok(vec![Claude]));
        assert_eq!(parse_agents("4, 2"), Ok(vec![Codex, Pi]));
        assert_eq!(parse_agents("cursor,Claude 3 1"), Ok(vec![Cursor, Claude]));
        for bad in ["0", "5", "vim", "1,x"] {
            assert!(parse_agents(bad).is_err(), "{bad}");
        }
    }
}
