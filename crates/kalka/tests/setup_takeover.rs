//! `kalka setup` takes over a project that `wiki` 0.6.13 (this tool's old name) set up: it removes
//! the old entries that are byte for byte the old tool's and leaves everything else.
//! Helpers copied from `setup.rs` (they are private there): the fixture, `setup`, `out`, `read`.

use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

/// Texts as `wiki setup` 0.6.13 wrote them (templates with its substitutions applied).
const OLD_SKILL: &str = include_str!("old-wiki/SKILL.md");
/// The skill an earlier 0.6.x wrote (`shipped/skill-8.md`), the one found on the bench.
const OLD_SKILL_8: &str = include_str!("old-wiki/skill-8.md");
const OLD_HOOK: &str = include_str!("old-wiki/index.ts");
const OLD_NAV: &str = include_str!("old-wiki/navigation.md");

struct Fixture {
    tmp: TempDir,
}

impl Fixture {
    fn root(&self) -> PathBuf {
        self.tmp.path().join("proj")
    }
    fn bin(&self) -> PathBuf {
        self.tmp.path().join("bin")
    }
    fn stub(&self, name: &str, body: &str) {
        let p = self.bin().join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn setup(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        let home = self.tmp.path().join("home");
        Command::cargo_bin("kalka")
            .unwrap()
            .arg("setup")
            .args(args)
            .current_dir(self.root())
            .env("PATH", self.bin())
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &home)
            .env("LOCALAPPDATA", &home)
            .env("CLAUDE_CONFIG_DIR", self.tmp.path().join("claude-config"))
            .assert()
    }
    fn write(&self, file: &str, text: &str) {
        let p = self.root().join(file);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    fn claude_log(&self) -> String {
        fs::read_to_string(self.tmp.path().join("claude.log")).unwrap_or_default()
    }
    /// Plants a local-scope server `name` running `cmd serve --source .` in the stub `claude`.
    fn claude_local(&self, name: &str, cmd: &str, scope: &str) {
        let text = format!(
            "{name}:\n  Scope: {scope}\n  Status: ok\n  Type: stdio\n  Command: {cmd}\n  Args: serve --source .\n"
        );
        fs::write(self.tmp.path().join(format!("get-{name}")), text).unwrap();
    }
}

/// A git repo with a `kalka` and a `pi` on PATH, and a `claude` that keeps one `mcp get` answer
/// per server name (`get-<name>`), as Claude Code does, and shows a `wiki` in `.mcp.json` as a
/// project-scope server.
fn fixture() -> Fixture {
    let f = Fixture {
        tmp: tempdir().unwrap(),
    };
    fs::create_dir_all(f.root().join("src")).unwrap();
    fs::create_dir_all(f.bin()).unwrap();
    fs::write(f.root().join("src/lib.rs"), "pub fn load_creds() {}\n").unwrap();
    std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(f.root())
        .status()
        .unwrap();
    symlink(env!("CARGO_BIN_EXE_kalka"), f.bin().join("kalka")).unwrap();
    f.stub("pi", "exit 0");
    let t = f.tmp.path().display();
    f.stub(
        "claude",
        &format!(
            r#"echo "$@" >> {t}/claude.log
case "$1 $2" in
"mcp get") if [ -f {t}/get-$3 ]; then /bin/cat {t}/get-$3
  elif [ "$3" = wiki ] && [ -f .mcp.json ] && /usr/bin/grep -q '"wiki"' .mcp.json; then
    printf 'wiki:\n  Scope: Project config (shared via .mcp.json)\n  Type: stdio\n  Command: wiki\n  Args: serve --source .\n'
  else echo "No MCP server named \"$3\"."; exit 1; fi ;;
"mcp add-json") printf '%s:\n  Scope: Local config\n  Type: stdio\n  Command: kalka\n  Args: serve --source .\n' "$3" > {t}/get-$3 ;;
"mcp remove") /bin/rm -f {t}/get-$3 ;;
esac"#
        ),
    );
    f
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

fn out(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(a.get_output().stdout.clone()).unwrap()
}

/// JSON as setup writes it.
fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap() + "\n"
}

fn old_entry() -> Value {
    json!({"command": "wiki", "args": ["serve", "--source", "."]})
}

fn old_pi_entry() -> Value {
    json!({"command": "wiki", "args": ["serve", "--source", "."], "exposure": "direct"})
}

fn old_hook() -> Value {
    json!({"hooks": [{"type": "command", "command": "wiki hint --claude-hook"}]})
}

fn old_block() -> String {
    format!("<!-- wiki:navigation -->\n{OLD_NAV}<!-- /wiki:navigation -->\n")
}

fn takeover_lines(o: &str) -> Vec<&str> {
    o.lines().filter(|l| l.contains("old wiki")).collect()
}

#[test]
fn setup_repo_removes_the_old_repo_scope_setup() {
    let f = fixture();
    let kalka = json!({"command": "kalka", "args": ["serve", "--source", "."]});
    let other = json!({"command": "x"});
    // A previous `kalka setup` already wrote its own entry beside the old one.
    f.write(
        ".mcp.json",
        &pretty(&json!({"mcpServers": {"other": other, "wiki": old_entry(), "kalka": kalka}})),
    );
    f.write(
        ".pi/mcp.json",
        &pretty(&json!({"mcpServers": {"wiki": old_pi_entry()}})),
    );
    f.write(
        ".cursor/mcp.json",
        &pretty(&json!({"mcpServers": {"wiki": old_entry()}})),
    );
    f.write(
        ".claude/settings.json",
        &pretty(&json!({"hooks": {"UserPromptSubmit": [old_hook()]}})),
    );
    f.write(".agents/skills/wiki-navigation/SKILL.md", OLD_SKILL);
    f.write(".pi/extensions/wiki-hint/index.ts", OLD_HOOK);
    f.write("AGENTS.md", &format!("# Team\n\n{}", old_block()));
    f.write(".gitignore", ".wiki/\n");

    let a = f
        .setup(&["--scope", "repo", "--agent", "claude,pi"])
        .success();
    let o = out(&a);
    assert_eq!(
        takeover_lines(&o),
        [
            "removed old wiki server from .mcp.json",
            "removed old wiki server from .pi/mcp.json",
            "removed old wiki server from .cursor/mcp.json",
            "removed old wiki hook from .claude/settings.json",
            "removed old wiki skill from .agents/skills/wiki-navigation/SKILL.md",
            "removed old wiki extension from .pi/extensions/wiki-hint/index.ts",
            "removed old wiki navigation block from AGENTS.md",
        ],
        "{o}"
    );
    // Before setup writes anything.
    assert!(
        o.starts_with("removed old wiki server from .mcp.json\n"),
        "{o}"
    );
    assert!(o.contains("already set up: .mcp.json kalka server"), "{o}");
    let r = f.root();
    assert_eq!(
        read(&r.join(".mcp.json")),
        pretty(&json!({"mcpServers": {"other": other, "kalka": kalka}}))
    );
    let pi: Value = serde_json::from_str(&read(&r.join(".pi/mcp.json"))).unwrap();
    assert!(pi["mcpServers"].get("wiki").is_none(), "{pi}");
    assert_eq!(pi["mcpServers"]["kalka"]["command"], "kalka");
    // Its last entry gone, the file and its folder go, as `--remove` does with its own.
    assert!(!r.join(".cursor").exists());
    assert_eq!(
        read(&r.join(".claude/settings.json")),
        pretty(&json!({"hooks": {"UserPromptSubmit": [
            {"hooks": [{"type": "command", "command": "kalka hint --claude-hook"}]}
        ]}}))
    );
    assert!(!r.join(".agents/skills/wiki-navigation").exists());
    assert!(!r.join(".pi/extensions/wiki-hint").exists());
    assert_eq!(read(&r.join("AGENTS.md")), "# Team\n");
    assert_eq!(read(&r.join(".gitignore")), ".wiki/\n.kulman/\n");

    // A second run finds nothing old.
    let a = f
        .setup(&["--scope", "repo", "--agent", "claude,pi"])
        .success();
    assert_eq!(takeover_lines(&out(&a)), Vec::<&str>::new());
}

/// The bench's mixed project: Claude's server in `.mcp.json` from a repo-scope run, everything
/// else from a local-scope run.
#[test]
fn setup_local_removes_a_mixed_old_setup() {
    let f = fixture();
    f.write(
        ".mcp.json",
        &pretty(&json!({"mcpServers": {"wiki": old_entry()}})),
    );
    f.write(
        ".pi/mcp.json",
        &pretty(&json!({"mcpServers": {"wiki": old_pi_entry()}})),
    );
    let settings =
        pretty(&json!({"permissions": {"allow": ["mcp__wiki__search", "mcp__wiki__page"]}}));
    f.write(".claude/settings.json", &settings);
    f.write(
        ".claude/settings.local.json",
        &pretty(&json!({
            "enabledMcpjsonServers": ["wiki"],
            "hooks": {"UserPromptSubmit": [old_hook()]}
        })),
    );
    f.write(".agents/skills/wiki-navigation/SKILL.md", OLD_SKILL_8);
    f.write(".pi/extensions/wiki-hint/index.ts", OLD_HOOK);
    f.write(".agents/memory/.keep", "");
    f.write(".wiki/index.json", "{}\n");
    let gitignore = ".claude/\n.mcp.json\n.wiki/\n";
    f.write(".gitignore", gitignore);
    let exclude = "# git ls-files --others --exclude-from=.git/info/exclude\n.wiki/\n/.agents/memory/\n/.pi/mcp.json\n/.agents/skills/wiki-navigation/\n/.pi/extensions/wiki-hint/\n/.claude/settings.local.json\n";
    f.write(".git/info/exclude", exclude);

    let a = f
        .setup(&["--scope", "local", "--agent", "claude,pi"])
        .success();
    let o = out(&a);
    assert_eq!(
        takeover_lines(&o),
        [
            "removed old wiki server from .mcp.json",
            "removed old wiki server from .pi/mcp.json",
            "removed old wiki hook from .claude/settings.local.json",
            "removed old wiki skill from .agents/skills/wiki-navigation/SKILL.md",
            "removed old wiki extension from .pi/extensions/wiki-hint/index.ts",
            "removed old wiki ignore line /.agents/skills/wiki-navigation/ from .git/info/exclude",
            "removed old wiki ignore line /.pi/extensions/wiki-hint/ from .git/info/exclude",
        ],
        "{o}"
    );
    let r = f.root();
    assert!(!r.join(".mcp.json").exists());
    assert_eq!(
        read(&r.join(".claude/settings.local.json")),
        pretty(&json!({
            "enabledMcpjsonServers": ["wiki"],
            "hooks": {"UserPromptSubmit": [
                {"hooks": [{"type": "command", "command": "kalka hint --claude-hook"}]}
            ]}
        }))
    );
    assert_eq!(read(&r.join(".claude/settings.json")), settings);
    assert!(!r.join(".agents/skills/wiki-navigation").exists());
    assert!(!r.join(".pi/extensions/wiki-hint").exists());
    assert!(r.join(".agents/memory/.keep").exists());
    assert_eq!(read(&r.join(".wiki/index.json")), "{}\n");
    assert_eq!(read(&r.join(".gitignore")), gitignore);
    let log = f.claude_log();
    assert!(!log.contains("mcp remove wiki"), "{log}");
    assert_eq!(
        read(&r.join(".git/info/exclude")),
        "# git ls-files --others --exclude-from=.git/info/exclude\n.wiki/\n/.agents/memory/\n/.pi/mcp.json\n/.claude/settings.local.json\n.kulman/\n/.agents/skills/kalka-navigation/\n/.pi/extensions/kalka-hint/\n"
    );
}

#[test]
fn setup_removes_the_old_local_claude_server_only() {
    let f = fixture();
    f.claude_local(
        "wiki",
        "wiki",
        "Local config (private to you in this project)",
    );
    let a = f.setup(&["--scope", "repo", "--agent", "claude"]).success();
    assert_eq!(
        takeover_lines(&out(&a)),
        ["removed old wiki server from claude (local scope)"]
    );
    assert!(f.claude_log().contains("mcp remove wiki --scope local"));

    // A user-scope `wiki` is not the project's: it stays.
    let g = fixture();
    g.claude_local(
        "wiki",
        "wiki",
        "User config (available in all your projects)",
    );
    let a = g.setup(&["--scope", "repo", "--agent", "claude"]).success();
    assert_eq!(takeover_lines(&out(&a)), Vec::<&str>::new());
    assert!(!g.claude_log().contains("mcp remove wiki"));
}

#[test]
fn setup_leaves_an_edited_old_skill_and_its_ignore_line() {
    let f = fixture();
    let edited = OLD_SKILL.replace("exact", "approximate");
    assert_ne!(edited, OLD_SKILL);
    f.write(".agents/skills/wiki-navigation/SKILL.md", &edited);
    f.write(".git/info/exclude", "/.agents/skills/wiki-navigation/\n");
    let block = old_block().replace("exact", "approximate");
    f.write("AGENTS.md", &block);
    let a = f.setup(&["--scope", "local", "--agent", "pi"]).success();
    assert_eq!(takeover_lines(&out(&a)), Vec::<&str>::new());
    let r = f.root();
    assert_eq!(
        read(&r.join(".agents/skills/wiki-navigation/SKILL.md")),
        edited
    );
    assert_eq!(read(&r.join("AGENTS.md")), block);
    let ex = read(&r.join(".git/info/exclude"));
    assert!(ex.starts_with("/.agents/skills/wiki-navigation/\n"), "{ex}");
}

/// No old traces: the output is what setup printed before the takeover existed.
#[test]
fn setup_without_old_traces_prints_what_it_printed_before() {
    let f = fixture();
    let a = f
        .setup(&["--scope", "local", "--agent", "claude,pi"])
        .success();
    let tmp = f.tmp.path().canonicalize().unwrap();
    let o = out(&a).replace(&tmp.display().to_string(), "<tmp>");
    let o = o.replace(&f.tmp.path().display().to_string(), "<tmp>");
    assert!(!o.contains("removed old wiki"), "{o}");
    let mut lines: Vec<&str> = o.lines().collect();
    // The agent memory folder's name is the mangled temporary path.
    let agent = lines.remove(1);
    assert!(
        agent.starts_with("agent memory folder <tmp>/claude-config/projects/")
            && agent.ends_with("-proj/memory (not yet created)"),
        "{agent}"
    );
    // Captured from a run on the commit before the takeover.
    assert_eq!(
        lines,
        [
            "wrote memory folder <tmp>/proj/.agents/memory",
            "Compiled 1 pages into .kulman",
            "wrote Claude Code local kalka server",
            "wrote .claude/settings.local.json prompt hook",
            "wrote .pi/mcp.json kalka server",
            "wrote .agents/skills/kalka-navigation/SKILL.md skill for pi",
            "wrote .pi/extensions/kalka-hint/index.ts hook for pi",
            "wrote .kulman/ in .git/info/exclude",
            "wrote /.agents/memory/ in .git/info/exclude",
            "wrote /.claude/settings.local.json in .git/info/exclude",
            "wrote /.pi/mcp.json in .git/info/exclude",
            "wrote /.agents/skills/kalka-navigation/ in .git/info/exclude",
            "wrote /.pi/extensions/kalka-hint/ in .git/info/exclude",
            "pi loads project files only after you trust the project (/trust, or --approve in print mode).",
        ]
    );
}

#[test]
fn a_failed_removal_still_prints_the_removals_before_it() {
    let f = fixture();
    f.write(
        ".mcp.json",
        &pretty(&json!({"mcpServers": {"wiki": old_entry()}})),
    );
    f.write(
        ".claude/settings.json",
        &pretty(&json!({"hooks": {"UserPromptSubmit": [old_hook()]}})),
    );
    let claude = f.root().join(".claude");
    let mode = |p: &Path, m| fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
    mode(&claude.join("settings.json"), 0o444);
    mode(&claude, 0o555);
    if fs::write(claude.join("settings.json"), "x").is_ok() {
        // root: permissions do not stop it, so there is no failed removal to see here.
        eprintln!("skipped: .claude/ stays writable for this user");
        return;
    }
    let a = f.setup(&["--scope", "repo", "--agent", "claude"]).code(1);
    mode(&claude, 0o755);
    assert!(!f.root().join(".mcp.json").exists());
    assert_eq!(
        takeover_lines(&out(&a)),
        ["removed old wiki server from .mcp.json"]
    );
}

#[test]
fn the_takeover_never_follows_a_link_out_of_the_project() {
    let f = fixture();
    let shared = f.tmp.path().join("shared");
    fs::create_dir_all(shared.join("wn")).unwrap();
    fs::write(shared.join("wn/SKILL.md"), OLD_SKILL).unwrap();
    let mcp = pretty(&json!({"mcpServers": {"wiki": old_entry()}}));
    fs::write(shared.join("mcp.json"), &mcp).unwrap();
    let r = f.root();
    fs::create_dir_all(r.join(".agents/skills")).unwrap();
    symlink(shared.join("wn"), r.join(".agents/skills/wiki-navigation")).unwrap();
    symlink(shared.join("mcp.json"), r.join(".mcp.json")).unwrap();
    let claude_md = format!("# Rules\n\n{}", old_block());
    f.write("CLAUDE.md", &claude_md);
    symlink("CLAUDE.md", r.join("AGENTS.md")).unwrap();

    let a = f.setup(&["--scope", "local", "--agent", "pi"]).success();
    assert_eq!(takeover_lines(&out(&a)), Vec::<&str>::new(), "{}", out(&a));
    assert_eq!(read(&shared.join("wn/SKILL.md")), OLD_SKILL);
    assert_eq!(read(&shared.join("mcp.json")), mcp);
    assert_eq!(read(&r.join("CLAUDE.md")), claude_md);
    assert!(r.join(".agents/skills/wiki-navigation").is_symlink());
    assert!(r.join("AGENTS.md").is_symlink());
}

/// The exclude file of a linked worktree lives in the main checkout's git folder.
#[test]
fn setup_removes_the_old_ignore_lines_from_a_worktree_exclude() {
    let f = fixture();
    let git = String::from_utf8(
        std::process::Command::new("which")
            .arg("git")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    symlink(git.trim(), f.bin().join("git")).unwrap();
    let main = f.root();
    let run = |args: &[&str]| {
        let s = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&main)
            .status()
            .unwrap();
        assert!(s.success());
    };
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "x"]);
    let wt = f.tmp.path().join("wt");
    run(&["worktree", "add", "-q", wt.to_str().unwrap()]);
    f.write(".git/info/exclude", "/.pi/extensions/wiki-hint/\n");
    let a = Command::cargo_bin("kalka")
        .unwrap()
        .args(["setup", "--agent", "pi", "--scope", "local"])
        .current_dir(&wt)
        .env("PATH", f.bin())
        .env("HOME", f.tmp.path().join("home"))
        .env("XDG_DATA_HOME", f.tmp.path().join("home"))
        .assert()
        .success();
    assert_eq!(
        takeover_lines(&out(&a)),
        ["removed old wiki ignore line /.pi/extensions/wiki-hint/ from .git/info/exclude"]
    );
    let ex = read(&main.join(".git/info/exclude"));
    assert!(!ex.contains("wiki-hint"), "{ex}");
}

#[test]
fn remove_leaves_the_old_entries_alone() {
    let f = fixture();
    f.stub("codex", "exit 1");
    let files = [
        (
            ".mcp.json",
            pretty(&json!({"mcpServers": {"wiki": old_entry()}})),
        ),
        ("AGENTS.md", old_block()),
        (
            ".agents/skills/wiki-navigation/SKILL.md",
            OLD_SKILL.to_string(),
        ),
    ];
    for (file, text) in &files {
        f.write(file, text);
    }
    let a = f
        .setup(&["--remove", "--scope", "repo", "--agent", "claude,pi,codex"])
        .success();
    assert_eq!(takeover_lines(&out(&a)), Vec::<&str>::new(), "{}", out(&a));
    for (file, text) in &files {
        assert_eq!(&read(&f.root().join(file)), text, "{file}");
    }
}

#[test]
fn an_old_local_claude_server_with_other_settings_stays() {
    let f = fixture();
    fs::write(
        f.tmp.path().join("get-wiki"),
        "wiki:\n  Scope: Local config (private to you in this project)\n  Status: ok\n  Type: stdio\n  Command: wiki\n  Args: serve --source .\n  Environment:\n    TOKEN=x\n",
    )
    .unwrap();
    let a = f.setup(&["--scope", "repo", "--agent", "claude"]).success();
    assert_eq!(takeover_lines(&out(&a)), Vec::<&str>::new());
    assert!(!f.claude_log().contains("mcp remove wiki"));
}

/// A file the takeover cannot read fails nothing that did not fail before.
#[test]
fn an_unreadable_old_location_does_not_fail_setup() {
    let f = fixture();
    fs::create_dir_all(f.root().join(".mcp.json")).unwrap();
    let a = f.setup(&["--scope", "local", "--agent", "pi"]).success();
    assert_eq!(takeover_lines(&out(&a)), Vec::<&str>::new());
    assert!(f.root().join(".mcp.json").is_dir());
}
