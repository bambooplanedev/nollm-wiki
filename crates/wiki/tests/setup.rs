//! `wiki setup` end to end, with a stub `claude` and a `PATH` of stub binaries only.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

const SKILL: &str = include_str!("../src/setup/SKILL.md");

/// The skill setup writes when the memory folder is `dir`.
fn skill(dir: &str) -> String {
    SKILL.replace("<memory-dir>", dir)
}

const ENTRY: &str = "{\n  \"command\": \"wiki\",\n  \"args\": [\n    \"serve\",\n    \"--source\",\n    \".\"\n  ]\n}";

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
    /// The calls to `claude`.
    fn claude_log(&self) -> String {
        fs::read_to_string(self.tmp.path().join("claude.log")).unwrap_or_default()
    }
    fn setup(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        self.run(Path::new(env!("CARGO_BIN_EXE_wiki")), args)
            .assert()
    }
    /// `exe setup args` in the fixture's environment.
    fn run(&self, exe: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(exe);
        c.arg("setup")
            .args(args)
            .current_dir(self.root())
            .env("PATH", self.bin())
            .env("HOME", self.home())
            .env_remove("KULMAN_HOME")
            .env("XDG_DATA_HOME", self.home())
            .env("LOCALAPPDATA", self.home())
            .env("CLAUDE_CONFIG_DIR", self.claude_config());
        c
    }
    /// wiki in the default kulman home, `$HOME/.kulman-home`.
    fn home_wiki(&self) -> PathBuf {
        wiki_in(&self.home().join(".kulman-home"))
    }
    fn home(&self) -> PathBuf {
        self.tmp.path().join("home")
    }
    fn claude_config(&self) -> PathBuf {
        self.tmp.path().join("claude-config")
    }
}

/// A copy of wiki at `home/bin/wiki`, where a kulman install puts it.
fn wiki_in(home: &Path) -> PathBuf {
    let exe = home.join("bin/wiki");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_wiki"), &exe).unwrap();
    exe
}

/// `with_wiki`: a `wiki` symlink on PATH; `with_claude`: the stub `claude`.
fn fixture(with_wiki: bool, with_claude: bool) -> Fixture {
    let f = Fixture {
        tmp: tempdir().unwrap(),
    };
    fs::create_dir_all(f.root().join("src")).unwrap();
    fs::create_dir_all(f.bin()).unwrap();
    fs::write(f.root().join("src/lib.rs"), "pub fn load_creds() {}\n").unwrap();
    // A normal environment: git is not on the stub PATH.
    std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(f.root())
        .status()
        .unwrap();
    if with_wiki {
        symlink(env!("CARGO_BIN_EXE_wiki"), f.bin().join("wiki")).unwrap();
    }
    f.stub("pi", "exit 0");
    if with_claude {
        claude_stub(&f, "wiki");
    }
    f
}

/// A `claude` that logs its argv and answers `mcp get wiki` (and no other name) as a local
/// server running `cmd` once `add-json` has been called (the state file), in the shape the real
/// CLI prints (2.1.325): fields indented by two, a blank line, the remove hint; not found is
/// exit 1 with the message on stderr.
fn claude_stub(f: &Fixture, cmd: &str) {
    claude_stub_args(f, cmd, "serve --source .");
}

/// [`claude_stub`] whose server runs `cmd` with `args`.
fn claude_stub_args(f: &Fixture, cmd: &str, args: &str) {
    let t = f.tmp.path().display();
    f.stub(
            "claude",
            &format!(
                r#"echo "$@" >> {t}/claude.log
case "$1 $2" in
"mcp get") [ "$3" = wiki ] && [ -f {t}/state ] || {{ echo "No MCP server named \"$3\". Configured servers: claude.ai Gmail" >&2; exit 1; }}
  printf 'wiki:\n  Scope: Local config (private to you in this project)\n  Status: ✔ Connected\n  Type: stdio\n  Command: {cmd}\n  Args: {args}\n\nTo remove this server, run: claude mcp remove wiki -s local\n' ;;
"mcp add-json") echo x > {t}/state ;;
"mcp remove") /bin/rm -f {t}/state ;;
esac"#
            ),
        );
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

fn out(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(a.get_output().stdout.clone()).unwrap()
}

fn dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let n = p.file_name().unwrap().to_string_lossy().into_owned();
        if p.is_dir() && n != ".git" && n != ".kulman" {
            v.extend(dirs(&p));
            v.push(p);
        }
    }
    v.sort();
    v
}

fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let n = p.file_name().unwrap().to_string_lossy().into_owned();
        if n == ".git" || n == ".kulman" {
            continue;
        }
        if p.is_dir() {
            v.extend(files(&p));
        } else {
            let b = fs::read(&p).unwrap();
            v.push((p, b));
        }
    }
    v.sort();
    v
}

#[test]
fn setup_repo_installs_mcp_skill_and_ignore() {
    let f = fixture(true, true);
    f.setup(&["--agent", "claude,pi", "--scope", "repo"])
        .success();
    let r = f.root();
    assert_eq!(
        read(&r.join(".mcp.json")),
        format!(
            "{{\n  \"mcpServers\": {{\n    \"wiki\": {}\n  }}\n}}\n",
            ENTRY.replace('\n', "\n    ")
        )
    );
    assert_eq!(
        read(&r.join(".pi/mcp.json")),
        format!(
            "{{\n  \"mcpServers\": {{\n    \"wiki\": {}\n  }}\n}}\n",
            ENTRY
                .replace("\n}", ",\n  \"exposure\": \"direct\"\n}")
                .replace('\n', "\n    ")
        )
    );
    assert!(!r.join(".claude/skills").exists());
    assert_eq!(
        read(&r.join(".agents/skills/wiki-navigation/SKILL.md")),
        skill(".agents/memory")
    );
    assert!(read(&r.join(".gitignore")).lines().any(|l| l == ".kulman/"));
    assert!(r.join(".kulman/index.json").exists());
}

#[test]
fn setup_twice_is_a_no_op() {
    let f = fixture(true, true);
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    f.setup(&args).success();
    let before = files(&f.root());
    let a = f.setup(&args).success();
    for l in out(&a)
        .lines()
        .filter(|l| !l.starts_with("Compiled") && !l.starts_with("agent memory folder"))
    {
        assert!(l.starts_with("already set up:"), "{l}");
    }
    assert_eq!(files(&f.root()), before);
}

#[test]
fn a_foreign_agents_md_navigation_block_survives_setup_and_remove_untouched() {
    let f = fixture(true, false);
    let agents = f.root().join("AGENTS.md");
    let foreign =
        "# Mine\n<!-- wiki:navigation -->\nsomeone else's rules\n<!-- /wiki:navigation -->\n";
    fs::write(&agents, foreign).unwrap();
    let a = f.setup(&["--agent", "codex", "--scope", "repo"]).code(1);
    assert!(out(&a).contains("skipped"), "{}", out(&a));
    assert_eq!(read(&agents), foreign);
    f.setup(&["--agent", "codex", "--scope", "repo", "--remove"]);
    assert_eq!(read(&agents), foreign);
}

#[test]
fn setup_leaves_a_foreign_wiki_entry_and_exits_1() {
    let f = fixture(true, true);
    let foreign = "{\"mcpServers\":{\"wiki\":{\"command\":\"other\"}}}";
    fs::write(f.root().join(".mcp.json"), foreign).unwrap();
    let a = f.setup(&["--agent", "claude", "--scope", "repo"]).code(1);
    assert_eq!(read(&f.root().join(".mcp.json")), foreign);
    assert!(out(&a).contains("skipped"));
    assert!(
        out(&a).contains("\"wiki\" has another value"),
        "{}",
        out(&a)
    );
}

#[test]
fn a_stale_foreign_wiki_entry_survives_setup_and_remove_untouched() {
    let f = fixture(true, true);
    let mcp = f.root().join(".mcp.json");
    let stale = "{\"mcpServers\":{\"wiki\":{\"command\":\"wiki\",\"args\":[\"serve\",\"--dir\",\"demo_wiki\"]}}}";
    fs::write(&mcp, stale).unwrap();
    let a = f.setup(&["--agent", "claude", "--scope", "repo"]).code(1);
    assert!(
        out(&a).contains("\"wiki\" has another value"),
        "{}",
        out(&a)
    );
    assert_eq!(read(&mcp), stale);
    f.setup(&[
        "--agent",
        "claude,pi,cursor,codex",
        "--scope",
        "repo",
        "--remove",
    ]);
    assert_eq!(read(&mcp), stale);
}

#[test]
fn setup_local_uses_claude_cli_and_git_exclude() {
    let f = fixture(true, true);
    f.setup(&["--agent", "claude,pi", "--scope", "local"])
        .success();
    let log = f.claude_log();
    assert!(log.contains("mcp add-json wiki"), "{log}");
    assert!(log.contains("--scope local"), "{log}");
    assert!(!f.root().join(".mcp.json").exists());
    let ex = read(&f.root().join(".git/info/exclude"));
    for l in [
        ".kulman/",
        "/.pi/mcp.json",
        "/.agents/skills/wiki-navigation/",
        "/.agents/memory/",
    ] {
        assert!(ex.lines().any(|x| x == l), "{l} missing in {ex}");
    }
    // Claude Code keeps its own memory, so it gets no skill.
    assert!(!f.root().join(".claude/skills").exists());
    assert!(!f.root().join(".gitignore").exists());
}

#[test]
fn setup_local_without_claude_cli_skips_and_exits_1() {
    let f = fixture(true, false);
    let a = f.setup(&["--agent", "claude", "--scope", "local"]).code(1);
    let o = out(&a);
    assert!(
        o.lines()
            .any(|l| l.starts_with("skipped") && l.contains("claude")),
        "{o}"
    );
}

#[test]
fn setup_remove_restores_the_project() {
    let f = fixture(true, true);
    let other =
        "{\n  \"mcpServers\": {\n    \"other\": {\n      \"command\": \"x\"\n    }\n  }\n}\n";
    fs::write(f.root().join(".mcp.json"), other).unwrap();
    let before = files(&f.root());
    let dirs_before = dirs(&f.root());
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    f.setup(&args).success();
    let a = f
        .setup(&["--remove", "--agent", "claude,pi", "--scope", "repo"])
        .success();
    let mut want = before;
    want.push((f.root().join(".gitignore"), b".kulman/\n".to_vec()));
    want.sort();
    assert_eq!(files(&f.root()), want);
    // The memory folder setup created stays, like any note in it.
    let mut want_dirs = dirs_before;
    want_dirs.extend([f.root().join(".agents"), f.root().join(".agents/memory")]);
    want_dirs.sort();
    assert_eq!(dirs(&f.root()), want_dirs);
    assert!(f.root().join(".kulman").exists());
    assert!(out(&a).contains("kept the ignore lines and .kulman/"));
}

#[test]
fn setup_without_scope_and_terminal_fails() {
    let f = fixture(true, true);
    let a = f.setup(&["--agent", "claude"]).failure();
    assert!(String::from_utf8_lossy(&a.get_output().stderr).contains("--scope repo"));
}

#[test]
fn setup_yes_is_the_run_without_it_and_asks_nothing() {
    let run = |yes: bool| {
        let f = fixture(true, false);
        let mut args = vec!["--scope", "local", "--agent", "pi"];
        if yes {
            args.push("--yes");
        }
        let a = f.setup(&args).success();
        // The temp folder's name and the memory folder's path under it differ per run.
        let tmp = f.tmp.path().canonicalize().unwrap().display().to_string();
        let o = out(&a)
            .lines()
            .filter(|l| !l.starts_with("agent memory folder"))
            .map(|l| l.replace(&tmp, "T"))
            .collect::<Vec<_>>();
        (o, tree(&f.root()))
    };
    let (yes, no) = (run(true), run(false));
    assert_eq!(yes.0, no.0);
    assert!(yes.1 == no.1, "the files differ");
    // As without a terminal, a missing --scope stays an error.
    let f = fixture(true, false);
    let a = f.setup(&["--agent", "pi", "--yes"]).failure();
    assert!(String::from_utf8_lossy(&a.get_output().stderr).contains("--scope repo"));
    let h = Command::cargo_bin("wiki")
        .unwrap()
        .args(["setup", "--help"])
        .assert()
        .success();
    assert!(out(&h).contains("--yes"), "{}", out(&h));
}

/// A fixture with no agent to detect: no agent folder, no `claude` or `pi` on `PATH`.
fn no_agent() -> Fixture {
    let f = fixture(true, false);
    fs::remove_file(f.bin().join("pi")).unwrap();
    f
}

const NO_AGENT: &str = "no agent set up: the wiki is in .kulman/; run wiki setup --agent … later";

#[test]
fn setup_with_no_agent_still_compiles_and_exits_0() {
    let f = no_agent();
    let a = f.setup(&["--scope", "repo"]).success();
    let err = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    assert_eq!(err.lines().collect::<Vec<_>>(), [NO_AGENT]);
    assert!(!out(&a).contains("not on PATH"), "{}", out(&a));
    let r = f.root();
    assert!(r.join(".kulman/index.json").exists());
    // What a setup with an agent writes besides the agent's own files, and nothing else.
    let (files, dirs) = tree(&r);
    assert_eq!(
        files,
        [
            (PathBuf::from(".gitignore"), b".kulman/\n".to_vec()),
            (
                PathBuf::from("src/lib.rs"),
                b"pub fn load_creds() {}\n".to_vec()
            )
        ]
    );
    assert_eq!(
        dirs,
        [
            PathBuf::from(".agents"),
            ".agents/memory".into(),
            "src".into()
        ]
    );
    // A later setup of an agent there, and its removal, end as they do in a fresh project.
    let fresh = no_agent();
    for args in [
        &["--agent", "cursor", "--scope", "repo"][..],
        &["--remove", "--agent", "cursor", "--scope", "repo"],
    ] {
        f.setup(args).success();
        fresh.setup(args).success();
        assert_eq!(tree(&f.root()), tree(&fresh.root()), "{args:?}");
    }
}

#[test]
fn setup_remove_with_no_agent_is_not_an_error() {
    let f = no_agent();
    f.setup(&["--scope", "local"]).success();
    let exclude = read(&f.root().join(".git/info/exclude"));
    let before = tree(&f.root());
    for _ in 0..2 {
        let a = f.setup(&["--remove", "--scope", "local"]).success();
        assert!(
            out(&a).contains("kept the ignore lines and .kulman/"),
            "{}",
            out(&a)
        );
        let err = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
        assert_eq!(err.lines().collect::<Vec<_>>(), [NO_AGENT_REMOVED]);
        assert_eq!(tree(&f.root()), before);
        assert_eq!(read(&f.root().join(".git/info/exclude")), exclude);
    }
    // An agent named: no such line.
    let a = f
        .setup(&["--remove", "--agent", "cursor", "--scope", "local"])
        .success();
    let err = String::from_utf8_lossy(&a.get_output().stderr).into_owned();
    assert!(!err.contains("no agent found"), "{err}");
}

const NO_AGENT_REMOVED: &str =
    "no agent found: nothing agent-specific removed; pass --agent … to remove one agent's wiring";

#[test]
fn setup_local_writes_wiki_and_warns_when_it_is_not_on_path() {
    let f = fixture(false, false);
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    assert!(out(&a).contains("not on PATH"));
    assert!(read(&f.root().join(".pi/mcp.json")).contains("\"command\": \"wiki\""));
}

#[test]
fn setup_does_not_take_another_program_called_wiki_for_this_one() {
    let f = fixture(false, false);
    f.stub("wiki", "echo 'Federated Wiki 0.1'");
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    let o = out(&a);
    assert!(o.contains("is not this wiki"), "{o}");
    assert!(
        o.contains(&f.bin().join("wiki").display().to_string()),
        "{o}"
    );
    // The first `wiki` on PATH is the one agents run, even with a real one further along.
    let f = fixture(true, false);
    let other = f.root().join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("wiki"), "#!/bin/sh\necho nope\n").unwrap();
    fs::set_permissions(other.join("wiki"), fs::Permissions::from_mode(0o755)).unwrap();
    let a = f
        .run(
            Path::new(env!("CARGO_BIN_EXE_wiki")),
            &["--agent", "pi", "--scope", "local"],
        )
        .env("PATH", format!("{}:{}", other.display(), f.bin().display()))
        .assert()
        .success();
    assert!(out(&a).contains("is not this wiki"), "{}", out(&a));
}

#[test]
fn setup_does_not_wait_for_a_wiki_that_stalls() {
    let f = fixture(false, false);
    f.stub("wiki", "while :; do :; done");
    let t = std::time::Instant::now();
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    assert!(t.elapsed().as_secs() < 10, "{:?}", t.elapsed());
    assert!(out(&a).contains("is not this wiki"), "{}", out(&a));
}

/// Puts the real `git` on the stub PATH.
fn real_git(f: &Fixture) {
    let out = std::process::Command::new("which")
        .arg("git")
        .output()
        .unwrap();
    let git = String::from_utf8(out.stdout).unwrap();
    symlink(git.trim(), f.bin().join("git")).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
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
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(s.success(), "git {args:?}");
}

#[test]
fn setup_remove_undoes_the_hooks() {
    let f = fixture(true, true);
    let r = f.root();
    fs::create_dir_all(r.join(".claude")).unwrap();
    fs::write(r.join(".claude/settings.json"), ECHO_HOOK).unwrap();
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    f.setup(&[&args[..], &["--hook"]].concat()).success();
    assert!(r.join(PI_EXT).exists());
    f.setup(&[&["--remove"], &args[..]].concat()).success();
    assert_eq!(read(&r.join(".claude/settings.json")), ECHO_HOOK);
    assert!(!r.join(".pi/extensions").exists());
    // No hooks installed: plain --remove still succeeds and says so.
    let a = f.setup(&[&["--remove"], &args[..]].concat()).success();
    assert!(out(&a).contains("not present: .pi/extensions/wiki-hint/index.ts hook for pi"));
}

#[test]
fn setup_local_finds_the_exclude_file_in_a_worktree_and_a_subdirectory() {
    let f = fixture(true, true);
    real_git(&f);
    let main = f.root();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-q", "-m", "x"]);
    let wt = f.tmp.path().join("wt");
    git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    let sub = main.join("src");
    for root in [&wt, &sub] {
        let a = Command::cargo_bin("wiki")
            .unwrap()
            .args(["setup", "--agent", "pi", "--scope", "local"])
            .current_dir(root)
            .env("PATH", f.bin())
            .env("HOME", f.home())
            .env("XDG_DATA_HOME", f.home())
            .env("LOCALAPPDATA", f.home())
            .assert()
            .success();
        assert!(!out(&a).contains("not a git repository"), "{}", out(&a));
    }
    let ex = read(&main.join(".git/info/exclude"));
    // Worktree top: lines unprefixed. Subdirectory: anchored to the worktree top, so prefixed.
    for l in [
        ".kulman/",
        "/.pi/mcp.json",
        "/src/.kulman/",
        "/src/.pi/mcp.json",
        "/src/.agents/skills/wiki-navigation/",
    ] {
        assert!(ex.lines().any(|x| x == l), "{l} missing from {ex}");
    }
    // Real git must agree: nothing setup wrote in the subdirectory shows as untracked.
    let st = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(&main)
        .output()
        .unwrap();
    let st = String::from_utf8(st.stdout).unwrap();
    assert!(st.trim().is_empty(), "setup files not excluded:\n{st}");
}

#[test]
fn setup_local_skips_a_file_git_tracks() {
    let f = fixture(true, true);
    real_git(&f);
    let r = f.root();
    fs::create_dir_all(r.join(".pi")).unwrap();
    let mcp = "{\n  \"mcpServers\": {}\n}\n";
    fs::write(r.join(".pi/mcp.json"), mcp).unwrap();
    git(&r, &["add", ".pi/mcp.json"]);
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).code(1);
    assert!(
        out(&a).contains("tracked by git; use --scope repo"),
        "{}",
        out(&a)
    );
    assert_eq!(read(&r.join(".pi/mcp.json")), mcp);
    assert!(r.join(".agents/skills/wiki-navigation/SKILL.md").exists());
}

#[test]
fn setup_local_twice_then_remove_goes_through_claude() {
    let f = fixture(true, true);
    let args = ["--agent", "claude", "--scope", "local"];
    f.setup(&args).success();
    let a = f.setup(&args).success();
    assert!(out(&a).contains("already set up: Claude Code local wiki server"));
    assert!(!f.claude_log().contains("mcp remove"));
    let a = f
        .setup(&["--remove", "--agent", "claude", "--scope", "local"])
        .success();
    assert!(out(&a).contains("removed Claude Code local wiki server"));
    assert!(f.claude_log().contains("mcp remove wiki --scope local"));
}

#[test]
fn setup_local_leaves_a_foreign_claude_server_alone() {
    let f = fixture(true, true);
    claude_stub(&f, "other");
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let args = ["--agent", "claude", "--scope", "local"];
    let a = f.setup(&args).code(1);
    assert!(out(&a).contains("skipped Claude Code local wiki server"));
    f.setup(&["--remove", "--agent", "claude", "--scope", "local"])
        .code(1);
    let log = f.claude_log();
    assert!(
        !log.contains("mcp add-json") && !log.contains("mcp remove"),
        "{log}"
    );
    // The skipped server leaves no exclude line; `.kulman/` still gets one.
    let ex = read(&f.root().join(".git/info/exclude"));
    assert!(ex.lines().any(|l| l == ".kulman/"), "{ex}");
}

#[test]
fn setup_local_reports_a_failed_claude_add() {
    let f = fixture(true, false);
    f.stub(
        "claude",
        "[ \"$2\" = get ] && { echo 'No MCP server named \"wiki\". Configured servers: ' >&2; exit 1; }\necho boom >&2\nexit 2",
    );
    let a = f.setup(&["--agent", "claude", "--scope", "local"]).code(1);
    let o = out(&a);
    assert!(
        o.contains("skipped Claude Code local wiki server: `claude mcp add-json` failed: boom"),
        "{o}"
    );
    assert!(!o.contains("wrote Claude Code"));
}

/// Pretty-printed as setup writes JSON, so a round trip is byte-equal.
const ECHO_HOOK: &str = "{\n  \"hooks\": {\n    \"UserPromptSubmit\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"echo hi\"\n          }\n        ]\n      }\n    ]\n  }\n}\n";

fn hook_item() -> serde_json::Value {
    serde_json::json!({"hooks": [{"type": "command", "command": "wiki hint --claude-hook"}]})
}

fn prompt_hooks(p: &Path) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(&read(p)).unwrap();
    v["hooks"]["UserPromptSubmit"].clone()
}

#[test]
fn setup_hook_adds_claude_hook_and_pi_extension() {
    let f = fixture(true, true);
    let r = f.root();
    fs::create_dir_all(r.join(".claude")).unwrap();
    fs::write(r.join(".claude/settings.json"), ECHO_HOOK).unwrap();
    f.setup(&["--agent", "claude,pi", "--scope", "repo", "--hook"])
        .success();
    let echo = serde_json::json!({"type": "command", "command": "echo hi"});
    assert_eq!(
        prompt_hooks(&r.join(".claude/settings.json")),
        serde_json::json!([{"hooks": [echo]}, hook_item()])
    );
    let ext = read(&r.join(".pi/extensions/wiki-hint/index.ts"));
    assert!(ext.contains("\"hint\"") && ext.contains("before_agent_start"));
    assert!(ext.contains("execFileSync(\"wiki\""));
}

#[test]
fn setup_local_hook_uses_settings_local() {
    let f = fixture(true, true);
    let r = f.root();
    f.setup(&["--agent", "claude,pi", "--scope", "local", "--hook"])
        .success();
    assert_eq!(
        prompt_hooks(&r.join(".claude/settings.local.json")),
        serde_json::json!([hook_item()])
    );
    assert!(!r.join(".claude/settings.json").exists());
    let ex = read(&r.join(".git/info/exclude"));
    for l in ["/.pi/extensions/wiki-hint/", "/.claude/settings.local.json"] {
        assert!(ex.lines().any(|x| x == l), "{l} missing in {ex}");
    }
}

#[test]
fn setup_installs_no_hook_without_the_hook_flag() {
    for extra in [&[][..], &["--no-hook"][..]] {
        let f = fixture(true, true);
        let r = f.root();
        let args = [&["--agent", "claude,pi", "--scope", "repo"][..], extra].concat();
        let a = f.setup(&args).success();
        assert!(!r.join(".claude/settings.json").exists());
        assert!(!r.join(".pi/extensions").exists());
        // Nothing was there to remove: no word about a hook.
        assert!(!out(&a).contains("hook"), "{}", out(&a));
    }
    let f = fixture(true, true);
    f.setup(&[
        "--agent",
        "claude",
        "--scope",
        "repo",
        "--hook",
        "--no-hook",
    ])
    .code(2)
    .stderr(predicates::str::contains("cannot be used with"));
}

/// The line a run without `--hook` prints for each hook an earlier setup wrote.
const HINT_OFF: &str =
    "the prompt hint is off by default now; run setup again with --hook to keep it";

#[test]
fn a_run_without_the_hook_flag_removes_the_hooks_an_earlier_setup_wrote() {
    for scope in ["repo", "local"] {
        let f = fixture(true, true);
        let r = f.root();
        let file = if scope == "repo" {
            ".claude/settings.json"
        } else {
            ".claude/settings.local.json"
        };
        fs::create_dir_all(r.join(".claude")).unwrap();
        fs::write(r.join(file), ECHO_HOOK).unwrap();
        let args = ["--agent", "claude,pi", "--scope", scope];
        f.setup(&[&args[..], &["--hook"]].concat()).success();
        let a = f.setup(&args).success();
        assert_eq!(read(&r.join(file)), ECHO_HOOK, "{scope}");
        assert!(!r.join(".pi/extensions").exists(), "{scope}");
        let o = out(&a);
        for what in [
            format!("{file} prompt hook"),
            format!("{PI_EXT} hook for pi"),
        ] {
            assert!(o.contains(&format!("removed {what}: {HINT_OFF}")), "{o}");
        }
        // Gone now: a further run says nothing about it.
        assert!(!out(&f.setup(&args).success()).contains("hook"), "{scope}");
    }
}

#[test]
fn a_run_without_the_hook_flag_leaves_an_unreadable_settings_file_and_succeeds() {
    // A file the run would not otherwise write does not fail it.
    for unreadable in [false, true] {
        let f = fixture(true, true);
        let r = f.root();
        let path = r.join(".claude/settings.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"x\": \"\xff\"}").unwrap();
        if unreadable {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        }
        let a = f
            .setup(&["--agent", "claude,pi", "--scope", "repo"])
            .success();
        let o = out(&a);
        let start = "left .claude/settings.json as it is: cannot check it for wiki prompt hook (";
        assert!(o.lines().any(|l| l.starts_with(start)), "{o}");
        // The rest of the run still happened.
        assert!(r.join(".pi/mcp.json").exists(), "{o}");
        assert!(read(&r.join(".gitignore")).lines().any(|l| l == ".kulman/"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"x\": \"\xff\"}");
    }
}

#[test]
fn removing_the_hook_takes_every_copy_of_it() {
    for remove in [false, true] {
        let f = fixture(true, true);
        let r = f.root();
        let path = r.join(".claude/settings.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let two = serde_json::json!({"hooks": {"UserPromptSubmit": [hook_item(), hook_item()]}});
        fs::write(&path, two.to_string()).unwrap();
        let mut args = vec!["--agent", "claude", "--scope", "repo"];
        if remove {
            args.push("--remove");
        }
        f.setup(&args).success();
        assert!(!path.exists(), "remove {remove}: {}", read(&path));
    }
}

#[test]
fn a_local_run_without_the_hook_flag_leaves_a_hook_git_tracks() {
    let f = fixture(true, true);
    real_git(&f);
    let r = f.root();
    // A teammate's repo-scope setup committed the pi hook.
    f.setup(&["--agent", "pi", "--scope", "repo", "--hook"])
        .success();
    git(&r, &["add", PI_EXT]);
    let hook = read(&r.join(PI_EXT));
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    assert_eq!(read(&r.join(PI_EXT)), hook);
    assert!(!out(&a).contains("hook"), "{}", out(&a));
}

#[test]
fn a_run_without_the_hook_flag_removes_its_own_pi_hook_and_leaves_an_edited_one() {
    let f = fixture(true, true);
    let r = f.root();
    let args = ["--agent", "pi", "--scope", "repo"];
    let path = r.join(PI_EXT);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let old = include_str!("../src/setup/wiki-hint.ts")
        .replace("<out>", ".kulman")
        .replace("<cmd>", "wiki");
    fs::write(&path, &old).unwrap();
    let a = f.setup(&args).success();
    assert!(!r.join(".pi/extensions").exists());
    assert!(out(&a).contains(HINT_OFF), "{}", out(&a));
    // Someone's own edit is not wiki's to remove, and not worth a skip.
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("{old}// mine\n")).unwrap();
    let a = f.setup(&args).success();
    assert!(path.exists());
    assert!(!out(&a).contains("hook"), "{}", out(&a));
}

#[test]
fn setup_repo_creates_the_memory_folder_and_the_skill_names_it() {
    let f = fixture(true, true);
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    let a = f.setup(&args).success();
    assert!(f.root().join(".agents/memory").is_dir());
    let o = out(&a);
    let with_prefix =
        |prefix: &str| -> Vec<&str> { o.lines().filter(|l| l.starts_with(prefix)).collect() };
    // The repo's own folder is made; the Claude folder for it is only named,
    // since nothing has written a note there yet.
    let made = with_prefix("wrote memory folder ");
    assert!(
        made.len() == 1 && made[0].ends_with("/.agents/memory"),
        "{o}"
    );
    let named = with_prefix("agent memory folder ");
    assert!(
        named.len() == 1 && named[0].ends_with("/memory (not yet created)"),
        "{o}"
    );
    let text = read(&f.root().join(".agents/skills/wiki-navigation/SKILL.md"));
    assert!(text.contains("`.agents/memory`"), "{text}");
    assert!(!text.contains("<memory-dir>"));
    // Re-running recognises its own skill and the folder.
    let b = f.setup(&args).success();
    assert!(
        out(&b).contains("already set up: memory folder"),
        "{}",
        out(&b)
    );
}

/// A fixture with real git and a commit.
fn committed() -> Fixture {
    let f = fixture(true, true);
    real_git(&f);
    git(&f.root(), &["add", "."]);
    git(&f.root(), &["commit", "-q", "-m", "x"]);
    f
}

#[test]
fn setup_and_serve_find_the_claude_memory_folder() {
    let f = committed();
    let slug = wiki::memory::claude_slug(&wiki::memory::display_path(
        &f.root().canonicalize().unwrap(),
    ));
    let mem = f.claude_config().join("projects").join(slug).join("memory");
    fs::create_dir_all(mem.join("logs")).unwrap();
    fs::write(mem.join("n.md"), "# Kelp\n\nKelp forest notes.\n").unwrap();
    fs::write(mem.join("logs/x.md"), "# Log\n\nsession log\n").unwrap();
    let shown = wiki::memory::display_path(&mem.canonicalize().unwrap());
    let a = f.setup(&["--agent", "pi", "--scope", "repo"]).success();
    assert!(
        out(&a).contains(&format!("agent memory folder {shown} (found)")),
        "{}",
        out(&a)
    );
    let index = read(&f.root().join(".kulman/index.json"));
    assert!(index.contains(&format!("{shown}/n.md")), "{index}");
    assert!(index.contains("\"memory\": true"), "{index}");
    assert!(!index.contains("logs/x.md"), "{index}");

    Command::cargo_bin("wiki")
        .unwrap()
        .args(["compile", ".", "out"])
        .current_dir(f.root())
        .env("CLAUDE_CONFIG_DIR", f.claude_config())
        .assert()
        .success();
    let plain = read(&f.root().join("out/index.json"));
    assert!(!plain.contains("n.md"), "{plain}");
}

#[test]
fn setup_without_the_claude_memory_folder_says_not_yet_created() {
    let f = committed();
    let a = f.setup(&["--agent", "pi", "--scope", "repo"]).success();
    assert!(out(&a).contains("(not yet created)"), "{}", out(&a));
}

#[test]
fn setup_local_creates_agents_memory_and_excludes_it() {
    let f = committed();
    let args = ["--agent", "pi", "--scope", "local"];
    f.setup(&args).success();
    let dir = f.root().join(".agents/memory");
    assert!(dir.is_dir());
    let ex = read(&f.root().join(".git/info/exclude"));
    assert!(ex.lines().any(|l| l == "/.agents/memory/"), "{ex}");
    assert_eq!(
        read(&f.root().join(".agents/skills/wiki-navigation/SKILL.md")),
        skill(".agents/memory")
    );
    let note = dir.join("tide.md");
    fs::write(&note, "# Tide table\n\nHarbour tide notes.\n").unwrap();
    f.setup(&args).success();
    let index = read(&f.root().join(".kulman/index.json"));
    assert!(index.contains(".agents/memory/tide.md"), "{index}");
    assert!(index.contains("\"memory\": true"), "{index}");
    f.setup(&[&["--remove"], &args[..]].concat()).success();
    assert!(note.is_file());
}

#[test]
fn setup_local_outside_git_still_creates_agents_memory() {
    let f = fixture(true, true);
    real_git(&f);
    fs::remove_dir_all(f.root().join(".git")).unwrap();
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    assert!(f.root().join(".agents/memory").is_dir());
    assert!(
        out(&a).contains("not a git repository: nothing to exclude"),
        "{}",
        out(&a)
    );
}

#[test]
fn setup_local_twice_reports_everything_already_there() {
    let f = committed();
    let args = ["--agent", "claude", "--scope", "local"];
    f.setup(&args).success();
    let o = out(&f.setup(&args).success());
    assert!(o.contains("already set up: memory folder"), "{o}");
    assert!(!o.contains("skipped"), "{o}");
}

/// A failed `mcp get` with nothing on stdout is no server, whatever stderr says first (a
/// warning, a reworded message): the entry is added, and a broken CLI fails there.
#[test]
fn setup_local_adds_the_server_after_a_get_with_a_warning_first() {
    let f = fixture(true, false);
    let t = f.tmp.path().display();
    f.stub(
        "claude",
        &format!(
            "echo \"$@\" >> {t}/claude.log\n[ \"$2\" = get ] || exit 0\necho 'warning: a newer version is available' >&2\necho 'No MCP server named \"wiki\".' >&2\nexit 1"
        ),
    );
    let a = f
        .setup(&["--agent", "claude", "--scope", "local"])
        .success();
    assert!(
        out(&a).contains("wrote Claude Code local wiki server"),
        "{}",
        out(&a)
    );
    assert!(
        f.claude_log().contains("mcp add-json wiki"),
        "{}",
        f.claude_log()
    );
}

/// A `codex` that logs its argv and answers `mcp get wiki` with the machine-wide server in
/// `codex.state` (`<command> <args>`; none until a test writes one), in the shape codex-cli 0.169
/// prints.
fn codex_stub(f: &Fixture) {
    let t = f.tmp.path().display();
    f.stub(
        "codex",
        &format!(
            r#"echo "$@" >> {t}/codex.log
case "$1 $2" in
"mcp get") [ -f {t}/codex.state ] || {{ echo "Error: No MCP server named 'wiki' found." >&2; exit 1; }}
  set -- $(/bin/cat {t}/codex.state); c=$1; shift
  printf 'wiki\n  enabled: true\n  transport: stdio\n  command: %s\n  args: %s\n  cwd: -\n  env: -\n  remove: codex mcp remove wiki\n' "$c" "$*" ;;
esac"#
        ),
    );
}

/// What setup writes into `.codex/config.toml`, with memory and without.
const CODEX_BLOCK: &str =
    "[mcp_servers.wiki]\ncommand = \"wiki\"\nargs = [\"serve\", \"--source\", \".\"]\n";
const CODEX_BLOCK_NO_MEMORY: &str = "[mcp_servers.wiki]\ncommand = \"wiki\"\nargs = [\"serve\", \"--source\", \".\", \"--memory\", \"none\"]\n";
/// The hint names the project's real path, as the process sees it.
fn codex_hint(f: &Fixture) -> String {
    format!("Codex loads the server only in a project it trusts: run codex in the project and accept the question, or add [projects.\"{}\"] trust_level = \"trusted\" to ~/.codex/config.toml", f.root().canonicalize().unwrap().display())
}
/// The fake `codex`'s calls.
fn codex_log(f: &Fixture) -> String {
    fs::read_to_string(f.tmp.path().join("codex.log")).unwrap_or_default()
}

const NAV: &str = include_str!("../src/setup/navigation.md");

#[test]
fn setup_cursor_writes_the_project_mcp_json_and_says_to_approve_it() {
    let f = fixture(true, false);
    let a = f.setup(&["--agent", "cursor", "--scope", "repo"]).success();
    assert_eq!(
        read(&f.root().join(".cursor/mcp.json")),
        format!(
            "{{\n  \"mcpServers\": {{\n    \"wiki\": {}\n  }}\n}}\n",
            ENTRY.replace('\n', "\n    ")
        )
    );
    let line = "Cursor: the editor lists the server under Settings → Tools & MCP; cursor-agent loads it only after `cursor-agent mcp enable wiki`.\n";
    assert!(out(&a).ends_with(line), "{}", out(&a));
    f.setup(&["--remove", "--agent", "cursor", "--scope", "repo"])
        .success();
    assert!(!f.root().join(".cursor").exists());
}

#[test]
fn setup_finds_cursor_by_its_project_folder() {
    let f = fixture(true, false);
    fs::remove_file(f.bin().join("pi")).unwrap();
    fs::create_dir_all(f.root().join(".cursor")).unwrap();
    f.setup(&["--scope", "repo"]).success();
    assert!(f.root().join(".cursor/mcp.json").exists());
}

#[test]
fn setup_codex_adds_its_server_and_the_steps_to_agents_md_and_remove_undoes_both() {
    let f = fixture(true, false);
    codex_stub(&f);
    let agents = f.root().join("AGENTS.md");
    fs::write(&agents, "# Team rules\n").unwrap();
    let args = ["--agent", "codex", "--scope", "repo"];
    let a = f.setup(&args).success();
    let o = out(&a);
    assert!(o.contains("wrote .codex/config.toml wiki server\n"), "{o}");
    assert!(o.ends_with(&format!("{}\n", codex_hint(&f))), "{o}");
    assert_eq!(read(&f.root().join(".codex/config.toml")), CODEX_BLOCK);
    // Nothing machine-wide to remove: only asked, never added.
    assert_eq!(codex_log(&f), "mcp get wiki\n");
    let block = format!("<!-- wiki:navigation -->\n{NAV}<!-- /wiki:navigation -->\n");
    assert_eq!(read(&agents), format!("# Team rules\n\n{block}"));

    let a = f.setup(&args).success();
    for l in out(&a).lines().filter(|l| {
        !l.starts_with("Compiled") && !l.starts_with("agent memory folder") && *l != codex_hint(&f)
    }) {
        assert!(l.starts_with("already set up:"), "{l}");
    }
    assert!(out(&a).contains("already set up: .codex/config.toml wiki server\n"));
    // Asked on every run; with no machine-wide entry, nothing is said.
    assert_eq!(codex_log(&f), "mcp get wiki\nmcp get wiki\n");

    let a = f
        .setup(&["--remove", "--agent", "codex", "--scope", "repo"])
        .success();
    assert!(
        out(&a).contains("removed .codex/config.toml wiki server\n"),
        "{}",
        out(&a)
    );
    assert!(!out(&a).contains(&codex_hint(&f)), "{}", out(&a));
    assert_eq!(read(&agents), "# Team rules\n");
    assert!(!f.root().join(".codex").exists());
    assert_eq!(codex_log(&f), "mcp get wiki\n".repeat(3));
}

#[test]
fn setup_codex_local_creates_agents_md_and_excludes_it() {
    let f = fixture(true, false);
    codex_stub(&f);
    let a = f.setup(&["--agent", "codex", "--scope", "local"]).success();
    assert!(f.root().join("AGENTS.md").exists());
    assert_eq!(read(&f.root().join(".codex/config.toml")), CODEX_BLOCK);
    let ex = read(&f.root().join(".git/info/exclude"));
    assert!(ex.lines().any(|l| l == "/AGENTS.md"), "{ex}");
    assert!(ex.lines().any(|l| l == "/.codex/config.toml"), "{ex}");
    assert!(
        out(&a).contains("wrote /.codex/config.toml in .git/info/exclude"),
        "{}",
        out(&a)
    );
}

#[test]
fn setup_codex_leaves_another_wiki_server_alone() {
    let f = fixture(true, false);
    let t = f.tmp.path().display();
    f.stub(
        "codex",
        &format!("echo \"$@\" >> {t}/codex.log\ncase \"$1 $2\" in \"mcp get\") printf 'wiki\\n  command: other\\n' ;; esac"),
    );
    let a = f.setup(&["--agent", "codex", "--scope", "repo"]).code(1);
    assert!(
        out(&a).contains(
            "skipped Codex wiki server (machine-wide): another \"wiki\" server is configured machine-wide and still runs in every other Codex session (`codex mcp remove wiki` removes it); this project's .codex/config.toml entry was written and takes precedence here\n"
        ),
        "{}",
        out(&a)
    );
    // Left machine-wide; the project's entry is written all the same.
    assert_eq!(codex_log(&f), "mcp get wiki\n");
    assert_eq!(read(&f.root().join(".codex/config.toml")), CODEX_BLOCK);
    assert!(out(&a).contains("wrote .codex/config.toml wiki server\n"));
}

#[test]
fn the_machine_wide_entry_is_asked_outside_the_project() {
    let f = fixture(true, false);
    let t = f.tmp.path().display();
    let inside = f.tmp.path().canonicalize().unwrap();
    // Codex's merged view: inside the fixture (a trusted parent project), wiki's exact entry;
    // outside it, none machine-wide.
    f.stub(
        "codex",
        &format!(
            r#"echo "$@" >> {t}/codex.log
case "$(/bin/pwd -P)" in "{}"*) printf '%s\n' '{}' ;;
*) echo "Error: No MCP server named 'wiki' found." >&2; exit 1 ;; esac"#,
            inside.display(),
            codex_get("serve --source .", "-")
        ),
    );
    let a = f.setup(&CODEX_ONLY).success();
    assert!(!out(&a).contains("machine-wide"), "{}", out(&a));
    assert_eq!(codex_log(&f), "mcp get wiki\n");
    assert_eq!(read(&f.root().join(".codex/config.toml")), CODEX_BLOCK);
}

/// `wiki setup` run with `home` as the home folder and `env` set, in the project.
fn setup_at_home(
    f: &Fixture,
    home: &Path,
    env: &[(&str, &Path)],
    args: &[&str],
) -> assert_cmd::assert::Assert {
    let mut c = Command::cargo_bin("wiki").unwrap();
    c.arg("setup")
        .args(args)
        .current_dir(f.root())
        .env("PATH", f.bin())
        .env("HOME", home)
        .env_remove("CODEX_HOME");
    for (k, v) in env {
        c.env(k, v);
    }
    c.assert()
}

#[test]
fn a_codex_home_reached_through_a_link_is_still_codexs() {
    let f = fixture(true, false);
    fs::create_dir_all(f.root().join(".codex")).unwrap();
    let link = f.tmp.path().join("codex-home");
    symlink(f.root().join(".codex"), &link).unwrap();
    let a = setup_at_home(&f, &f.home(), &[("CODEX_HOME", &link)], &CODEX_ONLY).code(1);
    assert!(out(&a).contains("Codex's own"), "{}", out(&a));
    assert!(!f.root().join(".codex/config.toml").exists());
}

#[test]
fn codexs_own_home_is_no_project_that_uses_codex() {
    let f = fixture(true, false);
    fs::remove_file(f.bin().join("pi")).unwrap();
    fs::create_dir_all(f.root().join(".codex")).unwrap();
    let a = setup_at_home(&f, &f.root(), &[], &["--scope", "repo"]).success();
    assert!(!out(&a).contains("codex"), "{}", out(&a));
    assert!(!f.root().join(".codex/config.toml").exists());
    // Named, it is said why nothing is written.
    let a = setup_at_home(&f, &f.root(), &[], &CODEX_ONLY).code(1);
    assert!(
        out(&a).contains("skipped .codex/config.toml wiki server: this .codex/ is Codex's own"),
        "{}",
        out(&a)
    );
    assert!(!f.root().join(".codex/config.toml").exists());
}

#[test]
fn setup_codex_adds_its_table_beside_a_server_that_names_wiki() {
    let f = fixture(true, false);
    let p = f.root().join(".codex/config.toml");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let proxy =
        "[mcp_servers.proxy]\ncommand = \"mcp-proxy\"\nargs = [\n  \"--name\",\n  \"wiki\",\n]\n";
    fs::write(&p, proxy).unwrap();
    f.setup(&CODEX_ONLY).success();
    assert_eq!(read(&p), format!("{proxy}\n{CODEX_BLOCK}"));
    f.setup(&[&["--remove"], &CODEX_ONLY[..]].concat())
        .success();
    assert_eq!(read(&p), proxy);
}

#[test]
fn an_unwritable_project_file_keeps_the_machine_wide_entry() {
    let f = fixture(true, false);
    codex_stub(&f);
    fs::write(f.tmp.path().join("codex.state"), "wiki serve --source .\n").unwrap();
    let dir = f.root().join(".codex");
    fs::create_dir_all(&dir).unwrap();
    let mode = |m| fs::set_permissions(&dir, fs::Permissions::from_mode(m)).unwrap();
    mode(0o555);
    if fs::write(dir.join("probe"), "").is_ok() {
        // root: permissions do not stop it, so there is no failed write to see here.
        fs::remove_file(dir.join("probe")).unwrap();
        mode(0o755);
        panic!("needs a user that cannot write to a mode 555 directory (not root)");
    }
    let a = f.setup(&CODEX_ONLY).failure();
    mode(0o755);
    assert!(!out(&a).contains("machine-wide"), "{}", out(&a));
    assert_eq!(codex_log(&f), "mcp get wiki\n");
    assert!(!dir.join("config.toml").exists());
}

#[test]
fn an_empty_codex_home_is_unset() {
    let f = fixture(true, false);
    let a = setup_at_home(&f, &f.root(), &[("CODEX_HOME", Path::new(""))], &CODEX_ONLY).code(1);
    assert!(out(&a).contains("Codex's own"), "{}", out(&a));
    assert!(!f.root().join(".codex").exists());
}

#[test]
fn a_codex_found_through_a_relative_path_entry_is_the_one_that_runs() {
    let f = fixture(true, false);
    let t = f.tmp.path().display();
    let dir = f.root().join("node_modules/.bin");
    fs::create_dir_all(&dir).unwrap();
    let stub = dir.join("codex");
    let get = codex_get("serve --source .", "-");
    fs::write(
        &stub,
        format!("#!/bin/sh\necho \"$@\" >> {t}/codex.log\n[ \"$2\" = get ] && printf '%s\\n' '{get}'\n:\n"),
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("node_modules/.bin:{}", f.bin().display());
    let a = Command::cargo_bin("wiki")
        .unwrap()
        .args(["setup", "--agent", "codex", "--scope", "repo"])
        .current_dir(f.root())
        .env("PATH", path)
        .env("HOME", f.home())
        .env_remove("CODEX_HOME")
        .assert()
        .code(1);
    // The project's stub answered from the temp dir, and its entry is reported.
    assert_eq!(codex_log(&f), "mcp get wiki\n");
    assert!(
        out(&a).contains(
            "skipped Codex wiki server (machine-wide): another \"wiki\" server is configured machine-wide and still runs in every other Codex session (`codex mcp remove wiki` removes it); this project's .codex/config.toml entry was written and takes precedence here\n"
        ),
        "{}",
        out(&a)
    );
}

#[test]
fn setup_codex_needs_no_codex_cli() {
    let f = fixture(true, false);
    let a = f.setup(&CODEX_ONLY).success();
    let o = out(&a);
    assert!(o.contains("wrote .codex/config.toml wiki server\n"), "{o}");
    assert!(o.ends_with(&format!("{}\n", codex_hint(&f))), "{o}");
    assert!(!o.contains("skipped"), "{o}");
    assert_eq!(read(&f.root().join(".codex/config.toml")), CODEX_BLOCK);
    f.setup(&[&["--remove"], &CODEX_ONLY[..]].concat())
        .success();
    assert!(!f.root().join(".codex").exists());
}

#[test]
fn setup_codex_edits_only_its_table_in_a_config_the_project_has() {
    let f = fixture(true, false);
    codex_stub(&f);
    let p = f.root().join(".codex/config.toml");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mine = "# team settings\r\nmodel = \"o3\"\r\n\r\n[mcp_servers.docs]\r\ncommand = \"docs\"";
    fs::write(&p, mine).unwrap();
    let local = ["--agent", "codex", "--scope", "local"];
    let a = f.setup(&[&local[..], &["--no-memory"]].concat()).success();
    assert!(
        out(&a).contains("wrote .codex/config.toml wiki server\n"),
        "{}",
        out(&a)
    );
    let crlf = |b: &str| format!("{mine}\r\n\r\n{}", b.replace('\n', "\r\n"));
    assert_eq!(read(&p), crlf(CODEX_BLOCK_NO_MEMORY));
    // The file was the project's: no exclude line for it.
    let ex = read(&f.root().join(".git/info/exclude"));
    assert!(!ex.contains(".codex"), "{ex}");

    let a = f.setup(&local).success();
    assert!(
        out(&a).contains("wrote .codex/config.toml wiki server\n"),
        "{}",
        out(&a)
    );
    assert_eq!(read(&p), crlf(CODEX_BLOCK));
    let a = f.setup(&local).success();
    assert!(
        out(&a).contains("already set up: .codex/config.toml wiki server\n"),
        "{}",
        out(&a)
    );

    let a = f.setup(&[&["--remove"], &local[..]].concat()).success();
    assert!(
        out(&a).contains("removed .codex/config.toml wiki server\n"),
        "{}",
        out(&a)
    );
    // Back byte for byte, but for the line end the file's last line had to get.
    assert_eq!(read(&p), format!("{mine}\r\n"));
    let a = f.setup(&[&["--remove"], &local[..]].concat()).success();
    assert!(
        out(&a).contains("not present: .codex/config.toml wiki server\n"),
        "{}",
        out(&a)
    );
}

#[test]
fn setup_codex_never_adds_a_second_wiki_definition() {
    let f = fixture(true, false);
    codex_stub(&f);
    fs::write(f.tmp.path().join("codex.state"), "wiki serve --source .\n").unwrap();
    let p = f.root().join(".codex/config.toml");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    for (text, why) in [
        (
            "[mcp_servers.\"wiki\"]\ncommand = \"wiki\"\nargs = [\"serve\", \"--source\", \".\"]\n",
            "another \"wiki\" server is configured",
        ),
        (
            "[mcp_servers.wiki]\ncommand = \"wiki\"\nargs = [\"serve\", \"--source\", \".\"]\nenabled = false\n",
            "another \"wiki\" server is configured",
        ),
        (
            "[mcp_servers]\nwiki = { command = \"wiki\" }\n",
            "another \"wiki\" server is configured",
        ),
        (
            "mcp_servers = {}\n",
            "the file defines mcp_servers inline, by dotted keys or as an array; add the wiki server by hand",
        ),
    ] {
        fs::write(&p, text).unwrap();
        for args in [&CODEX_ONLY[..], &[&["--remove"], &CODEX_ONLY[..]].concat()] {
            let a = f.setup(args).code(1);
            assert!(
                out(&a).contains(&format!(
                    "skipped .codex/config.toml wiki server: {why}\n"
                )),
                "{}",
                out(&a)
            );
            assert!(!out(&a).contains(&codex_hint(&f)), "{}", out(&a));
            assert_eq!(read(&p), text);
        }
    }
    // The project names a server: Codex would show that one, so the machine-wide one is left.
    assert_eq!(codex_log(&f), "");
}

#[test]
fn setup_codex_leaves_a_config_behind_a_link_or_unreadable() {
    let f = fixture(true, false);
    codex_stub(&f);
    let dir = f.root().join(".codex");
    let p = dir.join("config.toml");
    let outside = f.tmp.path().join("outside.toml");
    fs::write(&outside, "model = \"o3\"\n").unwrap();
    let link = "reached through a link, out of the project or a file with more than one name; left as it is";
    let unreadable = "not a regular UTF-8 file; left as it is";
    let cases: [(&str, &dyn Fn()); 5] = [
        ("symlink", &|| {
            fs::create_dir_all(&dir).unwrap();
            symlink(&outside, &p).unwrap();
        }),
        ("dir link", &|| symlink(f.tmp.path(), &dir).unwrap()),
        ("hard link", &|| {
            fs::create_dir_all(&dir).unwrap();
            fs::hard_link(&outside, &p).unwrap();
        }),
        ("not UTF-8", &|| {
            fs::create_dir_all(&dir).unwrap();
            fs::write(&p, b"model = \"\xff\"\n").unwrap();
        }),
        ("a folder", &|| fs::create_dir_all(&p).unwrap()),
    ];
    // What is at the place, byte for byte where it is a file.
    let snap = || {
        let kind = fs::symlink_metadata(&p).map(|m| m.file_type());
        (format!("{kind:?}"), fs::read(&p).ok())
    };
    for (name, make) in cases {
        make();
        let before = snap();
        let a = f.setup(&CODEX_ONLY).code(1);
        let why = if name.ends_with("link") {
            link
        } else {
            unreadable
        };
        assert!(
            out(&a).contains(&format!("skipped .codex/config.toml wiki server: {why}\n")),
            "{name}: {}",
            out(&a)
        );
        assert_eq!(snap(), before, "{name}");
        assert_eq!(read(&outside), "model = \"o3\"\n", "{name}");
        assert!(!f.tmp.path().join("config.toml").exists(), "{name}");
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.file_type().is_symlink() => fs::remove_file(&dir).unwrap(),
            _ => fs::remove_dir_all(&dir).unwrap(),
        }
    }
    assert_eq!(codex_log(&f), "");
}

#[test]
fn setup_local_skips_a_codex_config_git_tracks() {
    let f = fixture(true, false);
    codex_stub(&f);
    real_git(&f);
    fs::write(f.tmp.path().join("codex.state"), "wiki serve --source .\n").unwrap();
    let r = f.root();
    fs::create_dir_all(r.join(".codex")).unwrap();
    fs::write(r.join(".codex/config.toml"), "model = \"o3\"\n").unwrap();
    git(&r, &["add", ".codex/config.toml"]);
    let a = f.setup(&["--agent", "codex", "--scope", "local"]).code(1);
    assert!(
        out(&a)
            .contains("skipped .codex/config.toml wiki server: tracked by git; use --scope repo\n"),
        "{}",
        out(&a)
    );
    assert_eq!(read(&r.join(".codex/config.toml")), "model = \"o3\"\n");
    // Nothing written in the project: the machine-wide entry stays too.
    assert_eq!(codex_log(&f), "mcp get wiki\n");
    assert!(!out(&a).contains("machine-wide"), "{}", out(&a));
    let a = f.setup(&CODEX_ONLY).code(1);
    assert!(out(&a).contains("machine-wide"), "{}", out(&a));
}

#[test]
fn setup_finds_codex_by_its_project_folder() {
    let f = fixture(true, false);
    fs::remove_file(f.bin().join("pi")).unwrap();
    fs::create_dir_all(f.root().join(".codex")).unwrap();
    f.setup(&["--scope", "repo"]).success();
    assert_eq!(read(&f.root().join(".codex/config.toml")), CODEX_BLOCK);
}

#[test]
fn setup_codex_leaves_codexs_own_home_alone() {
    let f = fixture(true, false);
    codex_stub(&f);
    fs::write(f.tmp.path().join("codex.state"), "wiki serve --source .\n").unwrap();
    // The project is the home folder: its `.codex/` is Codex's machine-wide one.
    let a = Command::cargo_bin("wiki")
        .unwrap()
        .args(["setup", "--agent", "codex", "--scope", "repo"])
        .current_dir(f.root())
        .env("PATH", f.bin())
        .env("HOME", f.root())
        .env_remove("CODEX_HOME")
        .assert()
        .code(1);
    assert!(
        out(&a).contains("skipped .codex/config.toml wiki server: this .codex/ is Codex's own (CODEX_HOME or ~/.codex), whose config is machine-wide; left as it is\n"),
        "{}",
        out(&a)
    );
    assert!(!f.root().join(".codex").exists());
    assert_eq!(codex_log(&f), "");
    let a = Command::cargo_bin("wiki")
        .unwrap()
        .args(["setup", "--agent", "codex", "--scope", "repo"])
        .current_dir(f.root())
        .env("PATH", f.bin())
        .env("HOME", f.home())
        .env("CODEX_HOME", f.root().join(".codex"))
        .assert()
        .code(1);
    assert!(out(&a).contains("Codex's own"), "{}", out(&a));
    assert!(!f.root().join(".codex").exists());
}

#[test]
fn setup_codex_does_not_create_agents_md_beside_a_lone_claude_md() {
    let f = fixture(true, false);
    codex_stub(&f);
    fs::write(f.root().join("CLAUDE.md"), "# Project rules\n").unwrap();
    let a = f.setup(&["--agent", "codex", "--scope", "repo"]).code(1);
    assert!(!f.root().join("AGENTS.md").exists());
    assert_eq!(read(&f.root().join("CLAUDE.md")), "# Project rules\n");
    let o = out(&a);
    assert!(
        o.contains("skipped AGENTS.md wiki navigation steps: an AGENTS.md beside CLAUDE.md hides CLAUDE.md"),
        "{o}"
    );
    assert!(o.contains("wrote .codex/config.toml wiki server"), "{o}");

    // An AGENTS.md an earlier setup created beside CLAUDE.md goes.
    let block = format!("<!-- wiki:navigation -->\n{NAV}<!-- /wiki:navigation -->\n");
    fs::write(f.root().join("AGENTS.md"), &block).unwrap();
    let a = f.setup(&["--agent", "codex", "--scope", "repo"]).code(1);
    assert!(!f.root().join("AGENTS.md").exists(), "{}", out(&a));
    assert!(out(&a).contains("removed AGENTS.md"), "{}", out(&a));

    // With the team's own AGENTS.md there, the steps go into it as usual.
    fs::write(f.root().join("AGENTS.md"), "# Agents\n").unwrap();
    f.setup(&["--agent", "codex", "--scope", "repo"]).success();
    assert!(read(&f.root().join("AGENTS.md")).contains("<!-- wiki:navigation -->"));
}

#[test]
fn setup_keeps_hand_edited_block() {
    let f = fixture(true, false);
    codex_stub(&f);
    let agents = f.root().join("AGENTS.md");
    let edited = format!(
        "<!-- wiki:navigation -->\n{}<!-- /wiki:navigation -->\n",
        NAV.replace("exact", "approximate")
    );
    fs::write(&agents, &edited).unwrap();
    let a = f.setup(&["--agent", "codex", "--scope", "repo"]).code(1);
    assert_eq!(read(&agents), edited);
    assert!(out(&a).contains("holds another wiki block"), "{}", out(&a));
    let a = f
        .setup(&["--remove", "--agent", "codex", "--scope", "repo"])
        .code(1);
    assert_eq!(read(&agents), edited);
    assert!(out(&a).contains("holds another wiki block"), "{}", out(&a));

    let p = f.root().join(".agents/skills/wiki-navigation/SKILL.md");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mine = skill(".agents/memory") + "my line\n";
    fs::write(&p, &mine).unwrap();
    f.setup(&["--agent", "pi", "--scope", "repo"]).code(1);
    assert_eq!(read(&p), mine);
}

#[test]
fn the_search_first_rules_are_in_every_copy() {
    let agents = wiki::manifest::render_agents_md("proj");
    for (name, text) in [
        ("navigation.md", NAV),
        ("SKILL.md", SKILL),
        ("INSTRUCTIONS", wiki::serve::INSTRUCTIONS),
        ("render_agents_md", agents.as_str()),
    ] {
        assert!(text.contains("same turn as `search`"), "{name}");
        assert!(text.contains("`callers`"), "{name}");
        assert!(text.contains("every occurrence of a literal"), "{name}");
    }
}

#[test]
fn default_texts_name_only_default_tools() {
    for name in wiki::serve::TOOLS {
        if wiki::serve::DEFAULT_TOOLS.contains(&name) {
            continue;
        }
        for (text_name, text) in [
            ("INSTRUCTIONS", wiki::serve::INSTRUCTIONS),
            ("navigation.md", NAV),
            ("SKILL.md", SKILL),
        ] {
            assert!(
                !text.contains(&format!("`{name}`")),
                "{text_name} names `{name}`"
            );
        }
    }
}

#[test]
fn setup_waits_for_a_compile_holding_the_lock() {
    use std::sync::mpsc::channel;
    use std::time::Duration;
    let f = fixture(true, true);
    let root = f.root();
    fs::create_dir_all(root.join(".kulman/.serve")).unwrap();
    let (held_tx, held_rx) = channel();
    let (release_tx, release_rx) = channel::<()>();
    std::thread::scope(|s| {
        let root_ref = &root;
        s.spawn(move || {
            wiki::dirlock::exclusive(&root_ref.join(".kulman"), || {
                held_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(20));
            });
        });
        held_rx.recv().unwrap();
        let setup = s.spawn(|| {
            f.setup(&["--agent", "claude", "--scope", "repo"]).success();
        });
        std::thread::sleep(Duration::from_secs(1));
        assert!(!root.join(".kulman/index.json").exists());
        release_tx.send(()).unwrap();
        setup.join().unwrap();
    });
    assert!(root.join(".kulman/index.json").exists());
}

#[test]
fn setup_on_a_fresh_repo_creates_no_serve_dir() {
    let f = fixture(true, true);
    f.setup(&["--agent", "claude", "--scope", "repo"]).success();
    assert!(f.root().join(".kulman/index.json").exists());
    assert!(!f.root().join(".kulman/.serve").exists());
}

const ALL_AGENTS: [&str; 5] = [
    "--agent",
    "claude,pi,cursor,codex",
    "--scope",
    "repo",
    "--hook",
];

/// Files and directories below `root` (`.git` and `.kulman` left out), relative to it.
fn tree(root: &Path) -> (Vec<(PathBuf, Vec<u8>)>, Vec<PathBuf>) {
    let rel = |p: &Path| p.strip_prefix(root).unwrap().to_path_buf();
    (
        files(root).into_iter().map(|(p, b)| (rel(&p), b)).collect(),
        dirs(root).iter().map(|p| rel(p)).collect(),
    )
}

fn mcp_args(p: &Path) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(&read(p)).unwrap();
    serde_json::from_value(v["mcpServers"]["wiki"]["args"].clone()).unwrap()
}

#[test]
fn setup_no_memory_creates_no_folder_and_serves_without_memory() {
    let f = committed();
    codex_stub(&f);
    let r = f.root();
    fs::create_dir_all(r.join(".agents/memory")).unwrap();
    fs::write(r.join(".agents/memory/note.md"), "# Kelp\n\nKelp notes.\n").unwrap();
    let slug = wiki::memory::claude_slug(&wiki::memory::display_path(&r.canonicalize().unwrap()));
    let mem = f.claude_config().join("projects").join(slug).join("memory");
    fs::create_dir_all(&mem).unwrap();
    fs::write(mem.join("tide.md"), "# Tide\n\nTide notes.\n").unwrap();
    let o = out(&f
        .setup(&[&ALL_AGENTS[..], &["--no-memory"]].concat())
        .success());
    assert!(!o.contains("memory folder"), "{o}");
    for file in [".mcp.json", ".pi/mcp.json", ".cursor/mcp.json"] {
        assert!(
            mcp_args(&r.join(file)).ends_with(&["--memory".into(), "none".into()]),
            "{file}"
        );
    }
    assert_eq!(read(&r.join(".codex/config.toml")), CODEX_BLOCK_NO_MEMORY);
    let skill = read(&r.join(".agents/skills/wiki-navigation/SKILL.md"));
    assert!(!skill.contains("# Memory"), "{skill}");
    assert!(skill.ends_with("fall back to grep.\n"), "{skill}");
    let index = read(&r.join(".kulman/index.json"));
    assert!(
        !index.contains("note.md") && !index.contains("tide.md"),
        "{index}"
    );

    let fresh = fixture(true, true);
    fresh
        .setup(&["--agent", "claude,pi", "--scope", "local", "--no-memory"])
        .success();
    assert!(!fresh.root().join(".agents/memory").exists());
    let ex = read(&fresh.root().join(".git/info/exclude"));
    assert!(!ex.contains("memory"), "{ex}");
}

#[test]
fn setup_memory_flag_equals_the_default() {
    let (f, g) = (fixture(true, true), fixture(true, true));
    codex_stub(&f);
    codex_stub(&g);
    f.setup(&ALL_AGENTS).success();
    g.setup(&[&ALL_AGENTS[..], &["--memory"]].concat())
        .success();
    assert_eq!(tree(&f.root()), tree(&g.root()));
    let log = |f: &Fixture| read(&f.tmp.path().join("codex.log"));
    assert_eq!(log(&f), log(&g));
}

#[test]
fn setup_memory_and_no_memory_conflict() {
    let f = fixture(true, true);
    f.setup(&[&ALL_AGENTS[..], &["--memory", "--no-memory"]].concat())
        .code(2);
    assert!(!f.root().join(".agents").exists());
}

#[test]
fn setup_no_memory_then_default_turns_memory_on() {
    let no = [&ALL_AGENTS[..], &["--no-memory"]].concat();
    let once = |args: &[&str]| {
        let f = fixture(true, true);
        codex_stub(&f);
        f.setup(args).success();
        f
    };
    let (plain, bare) = (once(&ALL_AGENTS), once(&no));

    let f = once(&no);
    let a = f.setup(&ALL_AGENTS).success();
    assert!(!out(&a).contains("skipped"), "{}", out(&a));
    assert_eq!(tree(&f.root()), tree(&plain.root()));
    assert_eq!(read(&f.root().join(".codex/config.toml")), CODEX_BLOCK);

    let f = once(&ALL_AGENTS);
    fs::write(
        f.root().join(".agents/memory/note.md"),
        "# Kelp\n\nKelp notes.\n",
    )
    .unwrap();
    let a = f.setup(&no).success();
    assert!(!out(&a).contains("skipped"), "{}", out(&a));
    fs::remove_file(f.root().join(".agents/memory/note.md")).unwrap();
    let (files, mut dirs) = tree(&f.root());
    // Setup never deletes a memory folder.
    assert!(dirs.contains(&PathBuf::from(".agents/memory")));
    dirs.retain(|d| d != Path::new(".agents/memory"));
    assert_eq!((files, dirs), tree(&bare.root()));
    assert_eq!(
        read(&f.root().join(".codex/config.toml")),
        CODEX_BLOCK_NO_MEMORY
    );
    let index = read(&f.root().join(".kulman/index.json"));
    assert!(!index.contains("note.md"), "{index}");
}

#[test]
fn setup_remove_after_no_memory_restores_the_project() {
    let remove = [&["--remove"], &ALL_AGENTS[..]].concat();
    let run = |first: &[&str]| {
        let f = fixture(true, true);
        codex_stub(&f);
        f.setup(first).success();
        f.setup(&remove).success();
        assert!(!f.root().join(".codex").exists());
        f
    };
    let plain = run(&ALL_AGENTS);
    let no = run(&[&ALL_AGENTS[..], &["--no-memory"]].concat());
    let (files, mut dirs) = tree(&plain.root());
    dirs.retain(|d| !d.starts_with(".agents"));
    assert_eq!((files, dirs), tree(&no.root()));
}

#[test]
fn setup_local_no_memory_then_default_goes_through_claude() {
    let f = fixture(true, true);
    claude_stub_args(&f, "wiki", "serve --source . --memory none");
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let a = f
        .setup(&["--agent", "claude", "--scope", "local"])
        .success();
    assert!(
        out(&a).contains("wrote Claude Code local wiki server"),
        "{}",
        out(&a)
    );
    let log = f.claude_log();
    assert!(log.contains("mcp remove wiki --scope local"), "{log}");
    assert!(
        log.contains(
            r#"mcp add-json wiki {"command":"wiki","args":["serve","--source","."]} --scope local"#
        ),
        "{log}"
    );
}

/// A `codex` whose `mcp get wiki` always prints `get` (as `codex mcp get` does: stdio lines,
/// `cwd`, `env`, the remove hint).
fn codex_fixed(f: &Fixture, get: &str) {
    let t = f.tmp.path().display();
    f.stub(
        "codex",
        &format!(
            r#"echo "$@" >> {t}/codex.log
case "$1 $2" in
"mcp get") printf '%s\n' '{get}' ;;
esac"#
        ),
    );
}

fn codex_get(args: &str, env: &str) -> String {
    format!(
        "wiki\n  enabled: true\n  transport: stdio\n  command: wiki\n  args: {args}\n  cwd: -\n  env: {env}\n  remove: codex mcp remove wiki"
    )
}

const CODEX_ONLY: [&str; 4] = ["--agent", "codex", "--scope", "repo"];

#[test]
fn a_switch_leaves_a_codex_entry_with_other_settings_alone() {
    let f = fixture(true, false);
    codex_fixed(&f, &codex_get("serve --source .", "TOKEN=*****"));
    // wiki's command and args with an `env`: not what setup wrote, so never removed.
    let a = f
        .setup(&[&CODEX_ONLY[..], &["--no-memory"]].concat())
        .code(1);
    let o = out(&a);
    assert!(
        o.contains("skipped Codex wiki server (machine-wide): another \"wiki\" server is configured machine-wide and still runs in every other Codex session (`codex mcp remove wiki` removes it); this project's .codex/config.toml entry was written and takes precedence here\n"),
        "{o}"
    );
    assert_eq!(
        read(&f.root().join(".codex/config.toml")),
        CODEX_BLOCK_NO_MEMORY
    );
    // Asked on every run; setup reports it only where the project had no wiki of its own.
    let a = f.setup(&CODEX_ONLY).success();
    assert!(!out(&a).contains("skipped"), "{}", out(&a));
    // Every `--remove` notes it and leaves it, without changing the exit code.
    let left = "left the Codex wiki server (machine-wide): another \"wiki\" server that wiki did not write; `codex mcp remove wiki` removes it\n";
    for removed in ["removed", "not present:"] {
        let a = f
            .setup(&[&["--remove"], &CODEX_ONLY[..]].concat())
            .success();
        let o = out(&a);
        assert!(
            o.contains(&format!("{removed} .codex/config.toml wiki server\n{left}")),
            "{o}"
        );
    }
    assert_eq!(codex_log(&f), "mcp get wiki\n".repeat(4));
}

#[test]
fn a_switch_leaves_a_claude_entry_with_other_settings_alone() {
    let f = fixture(true, true);
    claude_stub_args(&f, "wiki", "serve --source .\n  Environment:\n    TOKEN=x");
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let a = f
        .setup(&["--agent", "claude", "--scope", "local", "--no-memory"])
        .code(1);
    assert!(
        out(&a).contains("skipped Claude Code local wiki server: wiki's server carries other settings; remove it with `claude mcp remove wiki --scope local`"),
        "{}",
        out(&a)
    );
    let log = f.claude_log();
    assert!(
        !log.contains("mcp remove") && !log.contains("add-json"),
        "{log}"
    );
}

#[test]
fn setup_local_default_then_no_memory_goes_through_claude() {
    let f = fixture(true, true);
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    f.setup(&["--agent", "claude", "--scope", "local", "--no-memory"])
        .success();
    let log = f.claude_log();
    assert!(log.contains("mcp remove wiki --scope local"), "{log}");
    assert!(
        log.contains(r#"mcp add-json wiki {"command":"wiki","args":["serve","--source",".","--memory","none"]} --scope local"#),
        "{log}"
    );
}

#[test]
fn setup_remove_takes_a_local_claude_entry_without_memory() {
    let f = fixture(true, true);
    claude_stub_args(&f, "wiki", "serve --source . --memory none");
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let a = f
        .setup(&["--remove", "--agent", "claude", "--scope", "local"])
        .success();
    assert!(
        out(&a).contains("removed Claude Code local wiki server"),
        "{}",
        out(&a)
    );
    assert!(f.claude_log().contains("mcp remove wiki --scope local"));
}

#[test]
fn setup_remove_ignores_the_memory_flags() {
    let run = |first: &[&str], flag: &str| {
        let f = fixture(true, true);
        codex_stub(&f);
        f.setup(first).success();
        f.setup(&[&["--remove", flag], &ALL_AGENTS[..]].concat())
            .success();
        assert!(!f.root().join(".codex").exists());
        let (files, mut dirs) = tree(&f.root());
        dirs.retain(|d| !d.starts_with(".agents"));
        (files, dirs)
    };
    let no = [&ALL_AGENTS[..], &["--no-memory"]].concat();
    let want = run(&ALL_AGENTS, "--memory");
    assert_eq!(run(&ALL_AGENTS, "--no-memory"), want);
    assert_eq!(run(&no, "--memory"), want);
    assert_eq!(run(&no, "--no-memory"), want);
}

// Run from the kulman home, setup starts wiki through the launch line: no `PATH`, and no one's
// home path in a file the team shares. These texts are what the agents run, pinned exactly.

const LAUNCH_ENTRY: &str = r#"{
  "command": "sh",
  "args": [
    "-c",
    "exec \"${KULMAN_HOME:-$HOME/.kulman-home}/bin/wiki\" serve --source ."
  ]
}"#;
/// The `Args:` line of `claude mcp get` for the launch entry: as stored, and as Claude Code
/// (2026-11-15) shows it, with `${VAR:-default}` printed as `${VAR}`.
const LAUNCH_ARGS: &str =
    r#"-c exec "${KULMAN_HOME:-$HOME/.kulman-home}/bin/wiki" serve --source ."#;
const LAUNCH_SHOWN: &str = r#"-c exec "${KULMAN_HOME}/bin/wiki" serve --source ."#;
/// The launch entry as `claude mcp add-json` gets it.
const LAUNCH_JSON: &str = r#"{"command":"sh","args":["-c","exec \"${KULMAN_HOME:-$HOME/.kulman-home}/bin/wiki\" serve --source ."]}"#;
const LAUNCH_HOOK: &str = r#""${KULMAN_HOME:-$HOME/.kulman-home}/bin/wiki" hint --claude-hook"#;
const LAUNCH_CODEX_BLOCK: &str = r#"[mcp_servers.wiki]
command = "sh"
args = ["-c", "exec \"${KULMAN_HOME:-$HOME/.kulman-home}/bin/wiki\" serve --source ."]
"#;
const LAUNCH_PI_HOOK: &str = r#"// Adds the project wiki's code lines for the prompt before the model runs (installed by wiki setup --hook).
import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  pi.on("before_agent_start", (event, ctx) => {
    if (event.messageType !== "user" || !event.prompt.trim()) return;
    let text = "";
    try {
      text = execFileSync(path.join(process.env.KULMAN_HOME || path.join(os.homedir(), ".kulman-home"), "bin/wiki"), ["hint", "--dir", path.join(ctx.cwd, ".kulman"), "--", event.prompt], {
        encoding: "utf8",
        timeout: 3000,
      });
    } catch {
      return;
    }
    if (!text.trim()) return;
    return { message: { customType: "wiki-hint", content: text, display: false } };
  });
}
"#;
const PI_EXT: &str = ".pi/extensions/wiki-hint/index.ts";
const KULMAN_HOME_WARNING: &str =
    "warning: the agents find wiki through KULMAN_HOME; set it in their environment";

/// An `mcpServers` file holding only `entry`, as setup writes it.
fn mcp_json(entry: &str) -> String {
    format!(
        "{{\n  \"mcpServers\": {{\n    \"wiki\": {}\n  }}\n}}\n",
        entry.replace('\n', "\n    ")
    )
}

fn launch_hook_item() -> serde_json::Value {
    serde_json::json!({"hooks": [{"type": "command", "command": LAUNCH_HOOK}]})
}

/// Every file of a repo-scope setup for all agents holds exactly the launch-line form, once.
fn assert_launch_files(r: &Path) {
    assert_eq!(read(&r.join(".mcp.json")), mcp_json(LAUNCH_ENTRY));
    let pi = LAUNCH_ENTRY.replace("\n}", ",\n  \"exposure\": \"direct\"\n}");
    assert_eq!(read(&r.join(".pi/mcp.json")), mcp_json(&pi));
    assert_eq!(read(&r.join(".cursor/mcp.json")), mcp_json(LAUNCH_ENTRY));
    assert_eq!(read(&r.join(".codex/config.toml")), LAUNCH_CODEX_BLOCK);
    assert_eq!(
        prompt_hooks(&r.join(".claude/settings.json")),
        serde_json::json!([launch_hook_item()])
    );
    assert_eq!(read(&r.join(PI_EXT)), LAUNCH_PI_HOOK);
}

#[test]
fn setup_from_the_kulman_home_writes_the_launch_line_and_no_path_warning() {
    let f = fixture(false, true);
    codex_stub(&f);
    let a = f.run(&f.home_wiki(), &ALL_AGENTS).assert().success();
    assert_launch_files(&f.root());
    let o = out(&a);
    assert!(o.contains("wrote .mcp.json wiki server"), "{o}");
    assert!(!o.contains("warning:"), "{o}");
}

#[test]
fn setup_local_from_the_kulman_home_adds_the_launch_line_through_claude() {
    for shown in [LAUNCH_SHOWN, LAUNCH_ARGS] {
        let f = fixture(false, true);
        claude_stub_args(&f, "sh", shown);
        let exe = f.home_wiki();
        let args = ["--agent", "claude", "--scope", "local", "--hook"];
        f.run(&exe, &args).assert().success();
        let added = format!("mcp get wiki\nmcp add-json wiki {LAUNCH_JSON} --scope local\n");
        assert_eq!(f.claude_log(), added);
        assert_eq!(
            prompt_hooks(&f.root().join(".claude/settings.local.json")),
            serde_json::json!([launch_hook_item()])
        );
        let a = f.run(&exe, &args).assert().success();
        assert!(
            out(&a).contains("already set up: Claude Code local wiki server"),
            "{shown}: {}",
            out(&a)
        );
        assert_eq!(f.claude_log(), format!("{added}mcp get wiki\n"));
        let a = f
            .run(&exe, &[&["--remove"], &args[..]].concat())
            .assert()
            .success();
        assert!(
            out(&a).contains("removed Claude Code local wiki server"),
            "{shown}: {}",
            out(&a)
        );
        assert_eq!(
            f.claude_log(),
            format!("{added}mcp get wiki\nmcp get wiki\nmcp remove wiki --scope local\n")
        );
        assert!(!f.root().join(".claude/settings.local.json").exists());
    }
}

#[test]
fn setup_local_from_the_kulman_home_switches_memory_on_the_shown_entry() {
    let f = fixture(false, true);
    claude_stub_args(&f, "sh", &format!("{LAUNCH_SHOWN} --memory none"));
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let args = ["--agent", "claude", "--scope", "local", "--no-hook"];
    f.run(&f.home_wiki(), &args).assert().success();
    assert_eq!(
        f.claude_log(),
        format!(
            "mcp get wiki\nmcp remove wiki --scope local\nmcp add-json wiki {LAUNCH_JSON} --scope local\n"
        )
    );
}

#[test]
fn setup_local_from_the_kulman_home_leaves_another_sh_server_alone() {
    let f = fixture(false, true);
    claude_stub_args(&f, "sh", r#"-c exec "/other/wiki" serve --source ."#);
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let exe = f.home_wiki();
    let args = ["--agent", "claude", "--scope", "local", "--no-hook"];
    let a = f.run(&exe, &args).assert().code(1);
    assert!(
        out(&a).contains(
            "skipped Claude Code local wiki server: another \"wiki\" server is configured"
        ),
        "{}",
        out(&a)
    );
    f.run(&exe, &[&["--remove"], &args[..]].concat())
        .assert()
        .code(1);
    assert_eq!(f.claude_log(), "mcp get wiki\nmcp get wiki\n");
}

#[test]
fn setup_local_from_the_kulman_home_replaces_a_bare_claude_entry() {
    let f = fixture(false, true);
    claude_stub(&f, "wiki");
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let args = ["--agent", "claude", "--scope", "local", "--no-hook"];
    f.run(&f.home_wiki(), &args).assert().success();
    assert_eq!(
        f.claude_log(),
        format!(
            "mcp get wiki\nmcp remove wiki --scope local\nmcp add-json wiki {LAUNCH_JSON} --scope local\n"
        )
    );
}

#[test]
fn setup_from_the_kulman_home_rewrites_a_bare_project_in_place_and_remove_takes_either_form() {
    let f = fixture(true, true);
    codex_stub(&f);
    let (r, exe) = (f.root(), f.home_wiki());
    let home = |args: &[&str]| f.run(&exe, args).assert().success();
    let remove = [&["--remove"], &ALL_AGENTS[..]].concat();
    // What a removal leaves: the project before setup, plus `.gitignore` and the memory folder.
    f.setup(&ALL_AGENTS).success();
    f.setup(&remove).success();
    let removed = tree(&r);
    // The bare form, from a wiki outside the home (one is in it), taken out from the home.
    f.setup(&ALL_AGENTS).success();
    assert_eq!(read(&r.join(".mcp.json")), mcp_json(ENTRY));
    home(&remove);
    assert_eq!(tree(&r), removed);
    // The bare form rewritten, one entry per file; then the launch form taken out.
    f.setup(&ALL_AGENTS).success();
    home(&ALL_AGENTS);
    assert_launch_files(&r);
    home(&remove);
    assert_eq!(tree(&r), removed);
}

#[test]
fn setup_outside_the_kulman_home_rewrites_a_launch_project_in_place_and_remove_takes_it() {
    let f = fixture(true, true);
    codex_stub(&f);
    let (r, exe) = (f.root(), f.home_wiki());
    let home = |args: &[&str]| f.run(&exe, args).assert().success();
    let remove = [&["--remove"], &ALL_AGENTS[..]].concat();
    f.setup(&ALL_AGENTS).success();
    let bare = tree(&r);
    f.setup(&remove).success();
    let removed = tree(&r);
    // The launch form, from a wiki outside the home: rewritten, one entry and one hook per file.
    home(&ALL_AGENTS);
    assert_launch_files(&r);
    f.setup(&ALL_AGENTS).success();
    assert_eq!(read(&r.join(".mcp.json")), mcp_json(ENTRY));
    assert_eq!(
        prompt_hooks(&r.join(".claude/settings.json")),
        serde_json::json!([hook_item()])
    );
    assert_eq!(tree(&r), bare);
    // The launch form taken out from outside the home.
    home(&ALL_AGENTS);
    f.setup(&remove).success();
    assert_eq!(tree(&r), removed);
}

#[test]
fn setup_local_outside_the_kulman_home_replaces_a_launch_claude_entry_and_remove_takes_it() {
    let f = fixture(true, true);
    claude_stub_args(&f, "sh", LAUNCH_SHOWN);
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let args = ["--agent", "claude", "--scope", "local", "--no-hook"];
    f.setup(&args).success();
    let set = "mcp get wiki\nmcp remove wiki --scope local\nmcp add-json wiki";
    assert_eq!(
        f.claude_log(),
        format!("{set} {{\"command\":\"wiki\",\"args\":[\"serve\",\"--source\",\".\"]}} --scope local\n")
    );
    f.setup(&[&["--remove"], &args[..]].concat()).success();
    assert!(f
        .claude_log()
        .ends_with("mcp get wiki\nmcp remove wiki --scope local\n"));
}

#[test]
fn setup_from_a_kulman_home_elsewhere_says_the_agents_need_kulman_home() {
    let f = fixture(false, true);
    let elsewhere = f.tmp.path().join("my \"kulman\" $home: x");
    let exe = wiki_in(&elsewhere);
    let args = ["--agent", "claude", "--scope", "repo"];
    let a = f
        .run(&exe, &args)
        .env("KULMAN_HOME", &elsewhere)
        .assert()
        .success();
    assert!(
        out(&a).lines().any(|l| l == KULMAN_HOME_WARNING),
        "{}",
        out(&a)
    );
    assert!(!out(&a).contains("not on PATH"), "{}", out(&a));
    assert_eq!(read(&f.root().join(".mcp.json")), mcp_json(LAUNCH_ENTRY));
    // `KULMAN_HOME` naming the default home: the agents find it without it.
    let default = f.home().join(".kulman-home");
    let a = f
        .run(&wiki_in(&default), &args)
        .env("KULMAN_HOME", &default)
        .assert()
        .success();
    let o = out(&a);
    assert!(o.contains("already set up: .mcp.json wiki server"), "{o}");
    assert!(!o.contains("warning:"), "{o}");
}

/// The `is ignored by git` lines of a setup report.
fn ignored_warnings(a: &assert_cmd::assert::Assert) -> Vec<String> {
    out(a)
        .lines()
        .filter(|l| l.contains("is ignored by git"))
        .map(String::from)
        .collect()
}

/// A project whose `.git/info/exclude` ignores `.claude/`.
fn ignoring_claude() -> Fixture {
    let f = fixture(true, true);
    fs::write(f.root().join(".git/info/exclude"), ".claude/\n").unwrap();
    f
}

const IGNORED: &str = "warning: .claude/settings.json is ignored by git; teammates will not get it";

#[test]
fn setup_repo_warns_for_each_written_file_git_ignores() {
    let f = ignoring_claude();
    real_git(&f);
    let a = f
        .setup(&["--agent", "claude,pi", "--scope", "repo", "--hook"])
        .success();
    // `.mcp.json` and the pi files are written too and not ignored: no warning for them.
    assert_eq!(ignored_warnings(&a), [IGNORED]);
    // Nothing written the second time, so nothing to warn about.
    let a = f
        .setup(&["--agent", "claude,pi", "--scope", "repo", "--hook"])
        .success();
    assert_eq!(ignored_warnings(&a), Vec::<String>::new());
}

#[test]
fn setup_does_not_warn_about_ignored_files_outside_a_repository() {
    // Not a repository: git exits 128.
    let f = ignoring_claude();
    real_git(&f);
    fs::remove_dir_all(f.root().join(".git")).unwrap();
    let a = f
        .setup(&["--agent", "claude", "--scope", "repo", "--hook"])
        .success();
    assert!(out(&a).contains("wrote .claude/settings.json prompt hook"));
    assert_eq!(ignored_warnings(&a), Vec::<String>::new());
}

#[test]
fn setup_local_does_not_warn_about_the_files_it_excludes_itself() {
    let f = ignoring_claude();
    real_git(&f);
    let a = f
        .setup(&["--agent", "claude", "--scope", "local", "--hook"])
        .success();
    assert!(out(&a).contains("wrote .claude/settings.local.json prompt hook"));
    assert_eq!(ignored_warnings(&a), Vec::<String>::new());
}

#[test]
fn setup_repo_makes_no_claude_call_at_all() {
    let f = fixture(true, true);
    let args = ["--agent", "claude", "--scope", "repo"];
    f.setup(&args).success();
    f.setup(&[&["--remove"], &args[..]].concat()).success();
    assert!(!f.tmp.path().join("claude.log").exists());
}
