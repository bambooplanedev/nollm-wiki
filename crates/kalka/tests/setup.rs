//! `kalka setup` end to end, with a stub `claude` and a `PATH` of stub binaries only.

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

const ENTRY: &str = "{\n  \"command\": \"kalka\",\n  \"args\": [\n    \"serve\",\n    \"--source\",\n    \".\"\n  ]\n}";

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
    fn claude_log(&self) -> String {
        fs::read_to_string(self.tmp.path().join("claude.log")).unwrap_or_default()
    }
    fn setup(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        Command::cargo_bin("kalka")
            .unwrap()
            .arg("setup")
            .args(args)
            .current_dir(self.root())
            .env("PATH", self.bin())
            .env("HOME", self.home())
            .env("XDG_DATA_HOME", self.home())
            .env("LOCALAPPDATA", self.home())
            .env("CLAUDE_CONFIG_DIR", self.claude_config())
            .assert()
    }
    fn home(&self) -> PathBuf {
        self.tmp.path().join("home")
    }
    fn claude_config(&self) -> PathBuf {
        self.tmp.path().join("claude-config")
    }
}

/// `with_wiki`: a `kalka` symlink on PATH; `with_claude`: the stub `claude`.
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
        symlink(env!("CARGO_BIN_EXE_kalka"), f.bin().join("kalka")).unwrap();
    }
    f.stub("pi", "exit 0");
    if with_claude {
        claude_stub(&f, "kalka");
    }
    f
}

/// A `claude` that logs its argv and answers `mcp get kalka` as a local server running `cmd`
/// once `add-json` has been called (the state file).
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
"mcp get") [ -f {t}/state ] || {{ echo 'No MCP server named "kalka".'; exit 1; }}
  printf 'kalka:\n  Scope: Local config (private to you in this project)\n  Status: ok\n  Type: stdio\n  Command: {cmd}\n  Args: {args}\n' ;;
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
            "{{\n  \"mcpServers\": {{\n    \"kalka\": {}\n  }}\n}}\n",
            ENTRY.replace('\n', "\n    ")
        )
    );
    assert_eq!(
        read(&r.join(".pi/mcp.json")),
        format!(
            "{{\n  \"mcpServers\": {{\n    \"kalka\": {}\n  }}\n}}\n",
            ENTRY
                .replace("\n}", ",\n  \"exposure\": \"direct\"\n}")
                .replace('\n', "\n    ")
        )
    );
    assert!(!r.join(".claude/skills").exists());
    assert_eq!(
        read(&r.join(".agents/skills/kalka-navigation/SKILL.md")),
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
fn setup_leaves_a_foreign_wiki_entry_and_exits_1() {
    let f = fixture(true, true);
    let foreign = "{\"mcpServers\":{\"kalka\":{\"command\":\"other\"}}}";
    fs::write(f.root().join(".mcp.json"), foreign).unwrap();
    let a = f.setup(&["--agent", "claude", "--scope", "repo"]).code(1);
    assert_eq!(read(&f.root().join(".mcp.json")), foreign);
    assert!(out(&a).contains("skipped"));
}

#[test]
fn setup_local_uses_claude_cli_and_git_exclude() {
    let f = fixture(true, true);
    f.setup(&["--agent", "claude,pi", "--scope", "local"])
        .success();
    let log = f.claude_log();
    assert!(log.contains("mcp add-json kalka"), "{log}");
    assert!(log.contains("--scope local"), "{log}");
    assert!(!f.root().join(".mcp.json").exists());
    let ex = read(&f.root().join(".git/info/exclude"));
    for l in [
        ".kulman/",
        "/.pi/mcp.json",
        "/.agents/skills/kalka-navigation/",
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
fn setup_without_agents_fails() {
    let f = fixture(true, false);
    fs::remove_file(f.bin().join("pi")).unwrap();
    let a = f.setup(&["--scope", "repo"]).failure();
    assert!(String::from_utf8_lossy(&a.get_output().stderr).contains("--agent"));
}

#[test]
fn setup_repo_writes_wiki_even_when_not_on_path() {
    let f = fixture(false, true);
    let a = f.setup(&["--agent", "claude", "--scope", "repo"]).success();
    assert!(out(&a).contains("not on PATH"));
    assert!(read(&f.root().join(".mcp.json")).contains("\"command\": \"kalka\""));
}

#[test]
fn setup_local_writes_wiki_and_warns_when_it_is_not_on_path() {
    let f = fixture(false, false);
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    assert!(out(&a).contains("not on PATH"));
    assert!(read(&f.root().join(".pi/mcp.json")).contains("\"command\": \"kalka\""));
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
fn setup_remove_undoes_the_default_hooks() {
    let f = fixture(true, true);
    let r = f.root();
    fs::create_dir_all(r.join(".claude")).unwrap();
    fs::write(r.join(".claude/settings.json"), ECHO_HOOK).unwrap();
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    f.setup(&args).success();
    f.setup(&[&["--remove"], &args[..]].concat()).success();
    assert_eq!(read(&r.join(".claude/settings.json")), ECHO_HOOK);
    assert!(!r.join(".pi/extensions").exists());
    // No hooks installed: plain --remove still succeeds and says so.
    let a = f.setup(&[&["--remove"], &args[..]].concat()).success();
    assert!(out(&a).contains("not present: .pi/extensions/kalka-hint/index.ts hook for pi"));
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
        let a = Command::cargo_bin("kalka")
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
        "/src/.agents/skills/kalka-navigation/",
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
    assert!(r.join(".agents/skills/kalka-navigation/SKILL.md").exists());
}

#[test]
fn setup_local_twice_then_remove_goes_through_claude() {
    let f = fixture(true, true);
    let args = ["--agent", "claude", "--scope", "local"];
    f.setup(&args).success();
    let a = f.setup(&args).success();
    assert!(out(&a).contains("already set up: Claude Code local kalka server"));
    assert!(!f.claude_log().contains("mcp remove"));
    let a = f
        .setup(&["--remove", "--agent", "claude", "--scope", "local"])
        .success();
    assert!(out(&a).contains("removed Claude Code local kalka server"));
    assert!(f.claude_log().contains("mcp remove kalka --scope local"));
}

#[test]
fn setup_local_leaves_a_foreign_claude_server_alone() {
    let f = fixture(true, true);
    claude_stub(&f, "other");
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let args = ["--agent", "claude", "--scope", "local"];
    let a = f.setup(&args).code(1);
    assert!(out(&a).contains("skipped Claude Code local kalka server"));
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
        "[ \"$2\" = get ] && exit 1\necho boom >&2\nexit 2",
    );
    let a = f.setup(&["--agent", "claude", "--scope", "local"]).code(1);
    let o = out(&a);
    assert!(
        o.contains("skipped Claude Code local kalka server: `claude mcp add-json` failed: boom"),
        "{o}"
    );
    assert!(!o.contains("wrote Claude Code"));
}

/// Pretty-printed as setup writes JSON, so a round trip is byte-equal.
const ECHO_HOOK: &str = "{\n  \"hooks\": {\n    \"UserPromptSubmit\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"echo hi\"\n          }\n        ]\n      }\n    ]\n  }\n}\n";

fn hook_item() -> serde_json::Value {
    serde_json::json!({"hooks": [{"type": "command", "command": "kalka hint --claude-hook"}]})
}

fn prompt_hooks(p: &Path) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(&read(p)).unwrap();
    v["hooks"]["UserPromptSubmit"].clone()
}

#[test]
fn setup_repo_adds_claude_hook_and_pi_extension_by_default() {
    let f = fixture(true, true);
    let r = f.root();
    fs::create_dir_all(r.join(".claude")).unwrap();
    fs::write(r.join(".claude/settings.json"), ECHO_HOOK).unwrap();
    f.setup(&["--agent", "claude,pi", "--scope", "repo"])
        .success();
    let echo = serde_json::json!({"type": "command", "command": "echo hi"});
    assert_eq!(
        prompt_hooks(&r.join(".claude/settings.json")),
        serde_json::json!([{"hooks": [echo]}, hook_item()])
    );
    let ext = read(&r.join(".pi/extensions/kalka-hint/index.ts"));
    assert!(ext.contains("\"hint\"") && ext.contains("before_agent_start"));
    assert!(ext.contains("execFileSync(\"kalka\""));
}

#[test]
fn setup_local_hook_uses_settings_local() {
    let f = fixture(true, true);
    let r = f.root();
    f.setup(&["--agent", "claude,pi", "--scope", "local"])
        .success();
    assert_eq!(
        prompt_hooks(&r.join(".claude/settings.local.json")),
        serde_json::json!([hook_item()])
    );
    assert!(!r.join(".claude/settings.json").exists());
    let ex = read(&r.join(".git/info/exclude"));
    for l in [
        "/.pi/extensions/kalka-hint/",
        "/.claude/settings.local.json",
    ] {
        assert!(ex.lines().any(|x| x == l), "{l} missing in {ex}");
    }
}

#[test]
fn setup_no_hook_installs_no_hook() {
    let f = fixture(true, true);
    let r = f.root();
    f.setup(&["--agent", "claude,pi", "--scope", "repo", "--no-hook"])
        .success();
    assert!(!r.join(".claude/settings.json").exists());
    assert!(!r.join(".pi/extensions").exists());
    // The old opt-in flag is gone.
    f.setup(&["--agent", "claude", "--scope", "repo", "--hook"])
        .failure();
}

#[test]
fn setup_hook_remove_keeps_other_hooks() {
    let f = fixture(true, true);
    let r = f.root();
    fs::create_dir_all(r.join(".claude")).unwrap();
    fs::write(r.join(".claude/settings.json"), ECHO_HOOK).unwrap();
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    f.setup(&args).success();
    f.setup(&[&["--remove"], &args[..]].concat()).success();
    assert_eq!(read(&r.join(".claude/settings.json")), ECHO_HOOK);
    assert!(!r.join(".pi/extensions").exists());
}

#[test]
fn setup_repo_creates_the_memory_folder_and_the_skill_names_it() {
    let f = fixture(true, true);
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    let a = f.setup(&args).success();
    assert!(f.root().join(".agents/memory").is_dir());
    let o = out(&a);
    assert!(o.contains("memory folder"), "{o}");
    assert!(
        o.contains("agent memory folder") || o.contains("no agent memory folders found"),
        "{o}"
    );
    let text = read(&f.root().join(".agents/skills/kalka-navigation/SKILL.md"));
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
    let slug = kalka::memory::claude_slug(&kalka::memory::display_path(
        &f.root().canonicalize().unwrap(),
    ));
    let mem = f.claude_config().join("projects").join(slug).join("memory");
    fs::create_dir_all(mem.join("logs")).unwrap();
    fs::write(mem.join("n.md"), "# Kelp\n\nKelp forest notes.\n").unwrap();
    fs::write(mem.join("logs/x.md"), "# Log\n\nsession log\n").unwrap();
    let shown = kalka::memory::display_path(&mem.canonicalize().unwrap());
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

    Command::cargo_bin("kalka")
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
        read(&f.root().join(".agents/skills/kalka-navigation/SKILL.md")),
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

/// A `codex` that logs its argv and answers `mcp get kalka` with the server `mcp add` added.
fn codex_stub(f: &Fixture) {
    let t = f.tmp.path().display();
    f.stub(
        "codex",
        &format!(
            r#"echo "$@" >> {t}/codex.log
case "$1 $2" in
"mcp get") [ -f {t}/codex.state ] || {{ echo "Error: No MCP server named 'kalka' found." >&2; exit 1; }}
  set -- $(/bin/cat {t}/codex.state); c=$1; shift
  printf 'kalka\n  enabled: true\n  transport: stdio\n  command: %s\n  args: %s\n' "$c" "$*" ;;
"mcp add") shift 4; echo "$@" > {t}/codex.state ;;
"mcp remove") /bin/rm -f {t}/codex.state ;;
esac"#
        ),
    );
}

const NAV: &str = include_str!("../src/setup/navigation.md");

#[test]
fn setup_cursor_writes_the_project_mcp_json_and_says_to_approve_it() {
    let f = fixture(true, false);
    let a = f.setup(&["--agent", "cursor", "--scope", "repo"]).success();
    assert_eq!(
        read(&f.root().join(".cursor/mcp.json")),
        format!(
            "{{\n  \"mcpServers\": {{\n    \"kalka\": {}\n  }}\n}}\n",
            ENTRY.replace('\n', "\n    ")
        )
    );
    assert!(
        out(&a).contains("cursor-agent mcp enable kalka"),
        "{}",
        out(&a)
    );
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
    f.setup(&args).success();
    let log = read(&f.tmp.path().join("codex.log"));
    assert!(
        log.contains("mcp add kalka -- kalka serve --source ."),
        "{log}"
    );
    let block = format!("<!-- kalka:navigation -->\n{NAV}<!-- /kalka:navigation -->\n");
    assert_eq!(read(&agents), format!("# Team rules\n\n{block}"));

    let a = f.setup(&args).success();
    for l in out(&a)
        .lines()
        .filter(|l| !l.starts_with("Compiled") && !l.starts_with("agent memory folder"))
    {
        assert!(l.starts_with("already set up:"), "{l}");
    }

    f.setup(&["--remove", "--agent", "codex", "--scope", "repo"])
        .success();
    assert_eq!(read(&agents), "# Team rules\n");
    assert!(read(&f.tmp.path().join("codex.log")).contains("mcp remove kalka"));
}

#[test]
fn setup_codex_local_creates_agents_md_and_excludes_it() {
    let f = fixture(true, false);
    codex_stub(&f);
    f.setup(&["--agent", "codex", "--scope", "local"]).success();
    assert!(f.root().join("AGENTS.md").exists());
    let ex = read(&f.root().join(".git/info/exclude"));
    assert!(ex.lines().any(|l| l == "/AGENTS.md"), "{ex}");
}

#[test]
fn setup_codex_leaves_another_wiki_server_alone() {
    let f = fixture(true, false);
    f.stub(
        "codex",
        "case \"$1 $2\" in \"mcp get\") printf 'kalka\\n  command: other\\n' ;; esac",
    );
    let a = f.setup(&["--agent", "codex", "--scope", "repo"]).code(1);
    assert!(
        out(&a).contains(
            "skipped Codex kalka server (~/.codex/config.toml): another \"kalka\" server"
        ),
        "{}",
        out(&a)
    );
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
        o.contains("skipped AGENTS.md kalka navigation steps: an AGENTS.md beside CLAUDE.md hides CLAUDE.md"),
        "{o}"
    );
    assert!(o.contains("wrote Codex kalka server"), "{o}");

    // An AGENTS.md an earlier setup created beside CLAUDE.md goes.
    let block = format!("<!-- kalka:navigation -->\n{NAV}<!-- /kalka:navigation -->\n");
    fs::write(f.root().join("AGENTS.md"), &block).unwrap();
    let a = f.setup(&["--agent", "codex", "--scope", "repo"]).code(1);
    assert!(!f.root().join("AGENTS.md").exists(), "{}", out(&a));
    assert!(out(&a).contains("removed AGENTS.md"), "{}", out(&a));

    // With the team's own AGENTS.md there, the steps go into it as usual.
    fs::write(f.root().join("AGENTS.md"), "# Agents\n").unwrap();
    f.setup(&["--agent", "codex", "--scope", "repo"]).success();
    assert!(read(&f.root().join("AGENTS.md")).contains("<!-- kalka:navigation -->"));
}

#[test]
fn setup_keeps_hand_edited_block() {
    let f = fixture(true, false);
    codex_stub(&f);
    let agents = f.root().join("AGENTS.md");
    let edited = format!(
        "<!-- kalka:navigation -->\n{}<!-- /kalka:navigation -->\n",
        NAV.replace("exact", "approximate")
    );
    fs::write(&agents, &edited).unwrap();
    let a = f.setup(&["--agent", "codex", "--scope", "repo"]).code(1);
    assert_eq!(read(&agents), edited);
    assert!(out(&a).contains("holds another kalka block"), "{}", out(&a));
    let a = f
        .setup(&["--remove", "--agent", "codex", "--scope", "repo"])
        .code(1);
    assert_eq!(read(&agents), edited);
    assert!(out(&a).contains("holds another kalka block"), "{}", out(&a));

    let p = f.root().join(".agents/skills/kalka-navigation/SKILL.md");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mine = skill(".agents/memory") + "my line\n";
    fs::write(&p, &mine).unwrap();
    f.setup(&["--agent", "pi", "--scope", "repo"]).code(1);
    assert_eq!(read(&p), mine);
}

#[test]
fn the_search_first_rules_are_in_every_copy() {
    let agents = kalka::manifest::render_agents_md("proj");
    for (name, text) in [
        ("navigation.md", NAV),
        ("SKILL.md", SKILL),
        ("INSTRUCTIONS", kalka::serve::INSTRUCTIONS),
        ("render_agents_md", agents.as_str()),
    ] {
        assert!(text.contains("same turn as `search`"), "{name}");
        assert!(text.contains("`callers`"), "{name}");
        assert!(text.contains("every occurrence of a literal"), "{name}");
    }
}

#[test]
fn default_texts_name_only_default_tools() {
    for name in kalka::serve::TOOLS {
        if kalka::serve::DEFAULT_TOOLS.contains(&name) {
            continue;
        }
        for (text_name, text) in [
            ("INSTRUCTIONS", kalka::serve::INSTRUCTIONS),
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
fn instructions_have_no_dangling_callers_clause() {
    let text = kalka::serve::INSTRUCTIONS;
    assert!(!text.contains("lists the functions that call it"), "{text}");
    assert!(
        text.contains("symbols with their lines. Do not call grep"),
        "{text}"
    );
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
            kalka::dirlock::exclusive(&root_ref.join(".kulman"), || {
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

/// A project that `wiki setup` set up in an earlier life: its `wiki` server, skill and block.
fn old_wiki_project() -> (Fixture, [(PathBuf, String); 3]) {
    let f = fixture(true, true);
    codex_stub(&f);
    let r = f.root();
    let mcp = "{\n  \"mcpServers\": {\n    \"wiki\": {\n      \"command\": \"wiki\",\n      \"args\": [\n        \"serve\",\n        \"--source\",\n        \".\"\n      ]\n    }\n  }\n}\n";
    let skill = "---\nname: wiki-navigation\n---\nold skill\n";
    let agents = "# Team\n\n<!-- wiki:navigation -->\nold steps\n<!-- /wiki:navigation -->\n";
    let old = [
        (r.join(".mcp.json"), mcp.to_string()),
        (
            r.join(".claude/skills/wiki-navigation/SKILL.md"),
            skill.into(),
        ),
        (r.join("AGENTS.md"), agents.into()),
    ];
    for (p, text) in &old {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    (f, old)
}

const ALL_AGENTS: [&str; 4] = ["--agent", "claude,pi,cursor,codex", "--scope", "repo"];

#[test]
fn setup_leaves_the_old_wiki_entries_alone() {
    let (f, old) = old_wiki_project();
    let a = f.setup(&ALL_AGENTS).success();
    let r = f.root();
    assert_eq!(read(&old[1].0), old[1].1, "{}", out(&a));
    let mcp: serde_json::Value = serde_json::from_str(&read(&old[0].0)).unwrap();
    let old_mcp: serde_json::Value = serde_json::from_str(&old[0].1).unwrap();
    assert_eq!(mcp["mcpServers"]["wiki"], old_mcp["mcpServers"]["wiki"]);
    assert_eq!(mcp["mcpServers"]["kalka"]["command"], "kalka");
    let agents = read(&old[2].0);
    assert!(agents.starts_with(&old[2].1), "{agents}");
    assert!(agents.contains("<!-- kalka:navigation -->"), "{agents}");
    assert!(r.join(".agents/skills/kalka-navigation/SKILL.md").exists());
    assert!(r.join(".cursor/mcp.json").exists());
}

#[test]
fn remove_takes_only_kalkas_entries() {
    let (f, old) = old_wiki_project();
    f.setup(&ALL_AGENTS).success();
    let a = f
        .setup(&[&["--remove"], &ALL_AGENTS[..]].concat())
        .success();
    let r = f.root();
    for (p, text) in &old {
        assert_eq!(&read(p), text, "{}", out(&a));
    }
    assert!(!r.join(".agents/skills/kalka-navigation").exists());
    assert!(!r.join(".cursor/mcp.json").exists());
}

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
    serde_json::from_value(v["mcpServers"]["kalka"]["args"].clone()).unwrap()
}

#[test]
fn setup_no_memory_creates_no_folder_and_serves_without_memory() {
    let f = committed();
    codex_stub(&f);
    let r = f.root();
    fs::create_dir_all(r.join(".agents/memory")).unwrap();
    fs::write(r.join(".agents/memory/note.md"), "# Kelp\n\nKelp notes.\n").unwrap();
    let slug = kalka::memory::claude_slug(&kalka::memory::display_path(&r.canonicalize().unwrap()));
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
    let log = read(&f.tmp.path().join("codex.log"));
    assert!(
        log.contains("mcp add kalka -- kalka serve --source . --memory none\n"),
        "{log}"
    );
    let skill = read(&r.join(".agents/skills/kalka-navigation/SKILL.md"));
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
    assert!(read(&f.tmp.path().join("codex.state")).ends_with("serve --source .\n"));

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
    assert!(read(&f.tmp.path().join("codex.state")).ends_with("--memory none\n"));
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
        assert!(!f.tmp.path().join("codex.state").exists());
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
    claude_stub_args(&f, "kalka", "serve --source . --memory none");
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let a = f
        .setup(&["--agent", "claude", "--scope", "local"])
        .success();
    assert!(
        out(&a).contains("wrote Claude Code local kalka server"),
        "{}",
        out(&a)
    );
    let log = f.claude_log();
    assert!(log.contains("mcp remove kalka --scope local"), "{log}");
    assert!(
        log.contains(r#"mcp add-json kalka {"command":"kalka","args":["serve","--source","."]} --scope local"#),
        "{log}"
    );
}

/// A `codex` whose `mcp get kalka` always prints `get` (as `codex mcp get` does: stdio lines,
/// `cwd`, `env`, the remove hint) and whose `mcp add` runs `add`.
fn codex_fixed(f: &Fixture, get: &str, add: &str) {
    let t = f.tmp.path().display();
    f.stub(
        "codex",
        &format!(
            r#"echo "$@" >> {t}/codex.log
case "$1 $2" in
"mcp get") printf '%s\n' '{get}' ;;
"mcp add") {add} ;;
esac"#
        ),
    );
}

fn codex_get(args: &str, env: &str) -> String {
    format!(
        "kalka\n  enabled: true\n  transport: stdio\n  command: kalka\n  args: {args}\n  cwd: -\n  env: {env}\n  remove: codex mcp remove kalka"
    )
}

const CODEX_ONLY: [&str; 4] = ["--agent", "codex", "--scope", "repo"];

#[test]
fn setup_switches_a_fixed_codex_entry_without_memory_to_the_default() {
    let f = fixture(true, false);
    codex_fixed(
        &f,
        &codex_get("serve --source . --memory none", "-"),
        "exit 0",
    );
    let a = f.setup(&CODEX_ONLY).success();
    assert!(out(&a).contains("wrote Codex kalka server"), "{}", out(&a));
    let log = read(&f.tmp.path().join("codex.log"));
    assert!(log.contains("mcp remove kalka\n"), "{log}");
    assert!(
        log.contains("mcp add kalka -- kalka serve --source .\n"),
        "{log}"
    );
}

#[test]
fn a_switch_leaves_a_codex_entry_with_other_settings_alone() {
    let f = fixture(true, false);
    codex_fixed(&f, &codex_get("serve --source .", "TOKEN=*****"), "exit 0");
    let a = f
        .setup(&[&CODEX_ONLY[..], &["--no-memory"]].concat())
        .code(1);
    let o = out(&a);
    assert!(
        o.contains("skipped Codex kalka server (~/.codex/config.toml): kalka's server carries other settings; remove it with `codex mcp remove kalka`"),
        "{o}"
    );
    let log = read(&f.tmp.path().join("codex.log"));
    assert!(
        !log.contains("mcp remove") && !log.contains("mcp add"),
        "{log}"
    );
    // The same choice leaves it alone as before; `--remove` still removes it.
    f.setup(&CODEX_ONLY).success();
    f.setup(&[&["--remove"], &CODEX_ONLY[..]].concat())
        .success();
    assert!(read(&f.tmp.path().join("codex.log")).contains("mcp remove kalka"));
}

#[test]
fn a_switch_leaves_a_claude_entry_with_other_settings_alone() {
    let f = fixture(true, true);
    claude_stub_args(&f, "kalka", "serve --source .\n  Environment:\n    TOKEN=x");
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let a = f
        .setup(&["--agent", "claude", "--scope", "local", "--no-memory"])
        .code(1);
    assert!(
        out(&a).contains("skipped Claude Code local kalka server: kalka's server carries other settings; remove it with `claude mcp remove kalka --scope local`"),
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
fn a_failed_add_after_a_switch_says_the_entry_was_removed() {
    let f = fixture(true, false);
    codex_fixed(
        &f,
        &codex_get("serve --source .", "-"),
        "echo boom >&2; exit 1",
    );
    let a = f
        .setup(&[&CODEX_ONLY[..], &["--no-memory"]].concat())
        .code(1);
    let o = out(&a);
    assert!(
        o.contains("skipped Codex kalka server (~/.codex/config.toml): kalka's server entry was removed and the new one could not be added (`codex mcp add` failed: boom); run `kalka setup` again to add it"),
        "{o}"
    );
}

#[test]
fn setup_local_default_then_no_memory_goes_through_claude() {
    let f = fixture(true, true);
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    f.setup(&["--agent", "claude", "--scope", "local", "--no-memory"])
        .success();
    let log = f.claude_log();
    assert!(log.contains("mcp remove kalka --scope local"), "{log}");
    assert!(
        log.contains(r#"mcp add-json kalka {"command":"kalka","args":["serve","--source",".","--memory","none"]} --scope local"#),
        "{log}"
    );
}

#[test]
fn setup_remove_takes_a_local_claude_entry_without_memory() {
    let f = fixture(true, true);
    claude_stub_args(&f, "kalka", "serve --source . --memory none");
    fs::write(f.tmp.path().join("state"), "x").unwrap();
    let a = f
        .setup(&["--remove", "--agent", "claude", "--scope", "local"])
        .success();
    assert!(
        out(&a).contains("removed Claude Code local kalka server"),
        "{}",
        out(&a)
    );
    assert!(f.claude_log().contains("mcp remove kalka --scope local"));
}

#[test]
fn setup_remove_ignores_the_memory_flags() {
    let run = |first: &[&str], flag: &str| {
        let f = fixture(true, true);
        codex_stub(&f);
        f.setup(first).success();
        f.setup(&[&["--remove", flag], &ALL_AGENTS[..]].concat())
            .success();
        assert!(!f.tmp.path().join("codex.state").exists());
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
