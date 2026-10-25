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

/// The memory-only skill setup writes for Claude Code.
fn memory_skill(dir: &str) -> String {
    let full = skill(dir);
    let (_, rules) = full.split_once("\n# Memory\n").unwrap();
    format!(
        "---\nname: wiki-memory\ndescription: Save or update a note in this project's memory: a decision, a fact or a journal entry. Use whenever something about this project should be remembered.\n---\n\n# Memory\n{rules}"
    )
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
    fn claude_log(&self) -> String {
        fs::read_to_string(self.tmp.path().join("claude.log")).unwrap_or_default()
    }
    fn setup(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        Command::cargo_bin("wiki")
            .unwrap()
            .arg("setup")
            .args(args)
            .current_dir(self.root())
            .env("PATH", self.bin())
            .env("HOME", self.home())
            .env("XDG_DATA_HOME", self.home())
            .env("LOCALAPPDATA", self.home())
            .assert()
    }
    fn home(&self) -> PathBuf {
        self.tmp.path().join("home")
    }
    /// Where local memory of the fixture's repo lives (needs real git and a commit).
    fn local_memory(&self) -> PathBuf {
        let env = |k: &str| {
            ["HOME", "XDG_DATA_HOME", "LOCALAPPDATA"]
                .contains(&k)
                .then(|| self.home().to_string_lossy().into_owned())
        };
        let data = wiki::memory::data_dir_for(std::env::consts::OS, &env).unwrap();
        data.join("wiki/memory")
            .join(wiki::memory::repo_key(&self.root()).unwrap())
    }
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

/// A `claude` that logs its argv and answers `mcp get wiki` as a local server running `cmd`
/// once `add-json` has been called (the state file).
fn claude_stub(f: &Fixture, cmd: &str) {
    let t = f.tmp.path().display();
    f.stub(
            "claude",
            &format!(
                r#"echo "$@" >> {t}/claude.log
case "$1 $2" in
"mcp get") [ -f {t}/state ] || {{ echo 'No MCP server named "wiki".'; exit 1; }}
  printf 'wiki:\n  Scope: Local config (private to you in this project)\n  Status: ok\n  Type: stdio\n  Command: {cmd}\n  Args: serve --source .\n' ;;
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
        if p.is_dir() && n != ".git" && n != ".wiki" {
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
        if n == ".git" || n == ".wiki" {
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
    assert_eq!(
        read(&r.join(".claude/skills/wiki-memory/SKILL.md")),
        memory_skill(".agents/memory")
    );
    assert!(!r.join(".claude/skills/wiki-navigation").exists());
    assert_eq!(
        read(&r.join(".agents/skills/wiki-navigation/SKILL.md")),
        skill(".agents/memory")
    );
    assert!(read(&r.join(".gitignore")).lines().any(|l| l == ".wiki/"));
    assert!(r.join(".wiki/index.json").exists());
}

#[test]
fn setup_replaces_an_untouched_claude_navigation_skill() {
    let f = fixture(true, true);
    let old = f.root().join(".claude/skills/wiki-navigation/SKILL.md");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::write(&old, skill(".agents/memory")).unwrap();
    let a = f.setup(&["--agent", "claude", "--scope", "repo"]).success();
    assert!(!old.exists(), "{}", out(&a));
    assert!(out(&a).contains("removed .claude/skills/wiki-navigation/SKILL.md"));
    assert!(f
        .root()
        .join(".claude/skills/wiki-memory/SKILL.md")
        .exists());
}

#[test]
fn setup_keeps_an_edited_claude_navigation_skill() {
    let f = fixture(true, true);
    let old = f.root().join(".claude/skills/wiki-navigation/SKILL.md");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::write(&old, "my own notes\n").unwrap();
    f.setup(&["--agent", "claude", "--scope", "repo"]).success();
    assert_eq!(read(&old), "my own notes\n");
}

#[test]
fn setup_twice_is_a_no_op() {
    let f = fixture(true, true);
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    f.setup(&args).success();
    let before = files(&f.root());
    let a = f.setup(&args).success();
    for l in out(&a).lines().filter(|l| !l.starts_with("Compiled")) {
        assert!(l.starts_with("already set up:"), "{l}");
    }
    assert_eq!(files(&f.root()), before);
}

#[test]
fn setup_leaves_a_foreign_wiki_entry_and_exits_1() {
    let f = fixture(true, true);
    let foreign = "{\"mcpServers\":{\"wiki\":{\"command\":\"other\"}}}";
    fs::write(f.root().join(".mcp.json"), foreign).unwrap();
    let a = f.setup(&["--agent", "claude", "--scope", "repo"]).code(1);
    assert_eq!(read(&f.root().join(".mcp.json")), foreign);
    assert_eq!(
        read(&f.root().join(".claude/skills/wiki-memory/SKILL.md")),
        memory_skill(".agents/memory")
    );
    assert!(out(&a).contains("skipped"));
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
        ".wiki/",
        "/.pi/mcp.json",
        "/.agents/skills/wiki-navigation/",
    ] {
        assert!(ex.lines().any(|x| x == l), "{l} missing in {ex}");
    }
    // No local memory here (no commit), so Claude Code gets no skill.
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
    want.push((f.root().join(".gitignore"), b".wiki/\n".to_vec()));
    want.sort();
    assert_eq!(files(&f.root()), want);
    // The memory folder setup created stays, like any note in it.
    let mut want_dirs = dirs_before;
    want_dirs.extend([f.root().join(".agents"), f.root().join(".agents/memory")]);
    want_dirs.sort();
    assert_eq!(dirs(&f.root()), want_dirs);
    assert!(f.root().join(".wiki").exists());
    assert!(out(&a).contains("kept the ignore lines and .wiki/"));
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
    assert!(read(&f.root().join(".mcp.json")).contains("\"command\": \"wiki\""));
}

#[test]
fn setup_local_writes_wiki_and_warns_when_it_is_not_on_path() {
    let f = fixture(false, false);
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    assert!(out(&a).contains("not on PATH"));
    assert!(read(&f.root().join(".pi/mcp.json")).contains("\"command\": \"wiki\""));
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
fn setup_hook_remove_without_hook_flag_undoes_hooks() {
    let f = fixture(true, true);
    let r = f.root();
    fs::create_dir_all(r.join(".claude")).unwrap();
    fs::write(r.join(".claude/settings.json"), ECHO_HOOK).unwrap();
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    f.setup(&[&args[..], &["--hook"]].concat()).success();
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
        ".wiki/",
        "/.pi/mcp.json",
        "/src/.wiki/",
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
    // The skipped server leaves no exclude line; `.wiki/` still gets one.
    let ex = read(&f.root().join(".git/info/exclude"));
    assert!(ex.lines().any(|l| l == ".wiki/"), "{ex}");
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
fn setup_hook_repo_adds_claude_hook_and_pi_extension() {
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
fn setup_hook_local_uses_settings_local() {
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
fn setup_hook_remove_keeps_other_hooks() {
    let f = fixture(true, true);
    let r = f.root();
    fs::create_dir_all(r.join(".claude")).unwrap();
    fs::write(r.join(".claude/settings.json"), ECHO_HOOK).unwrap();
    let args = ["--agent", "claude,pi", "--scope", "repo", "--hook"];
    f.setup(&args).success();
    f.setup(&[
        "--remove",
        "--agent",
        "claude,pi",
        "--scope",
        "repo",
        "--hook",
    ])
    .success();
    assert_eq!(read(&r.join(".claude/settings.json")), ECHO_HOOK);
    assert!(!r.join(".pi/extensions").exists());
}

#[test]
fn setup_repo_creates_the_memory_folder_and_the_skill_names_it() {
    let f = fixture(true, true);
    let a = f.setup(&["--agent", "claude", "--scope", "repo"]).success();
    assert!(f.root().join(".agents/memory").is_dir());
    assert!(out(&a).contains("memory folder"), "{}", out(&a));
    let text = read(&f.root().join(".claude/skills/wiki-memory/SKILL.md"));
    assert!(text.contains("`.agents/memory`"), "{text}");
    assert!(!text.contains("<memory-dir>"));
    // Re-running recognises its own skill and the folder.
    let b = f.setup(&["--agent", "claude", "--scope", "repo"]).success();
    assert!(
        out(&b).contains("already set up: memory folder"),
        "{}",
        out(&b)
    );
}

/// A fixture with real git and a commit, so local memory has a key.
fn committed() -> Fixture {
    let f = fixture(true, true);
    real_git(&f);
    git(&f.root(), &["add", "."]);
    git(&f.root(), &["commit", "-q", "-m", "x"]);
    f
}

#[test]
fn setup_local_creates_the_data_dir_folder_compiles_it_and_remove_keeps_it() {
    let f = committed();
    let dir = f.local_memory();
    let note = dir.join("tide.md");
    fs::create_dir_all(&dir).unwrap();
    fs::write(&note, "# Tide table\n\nHarbour tide notes.\n").unwrap();
    let a = f
        .setup(&["--agent", "claude", "--scope", "local"])
        .success();
    assert!(!f.root().join(".agents/memory").exists());
    let shown = wiki::memory::display_path(&dir.canonicalize().unwrap());
    let text = read(&f.root().join(".claude/skills/wiki-memory/SKILL.md"));
    assert!(
        text.contains(&format!("`{}`", wiki::memory::display_path(&dir))),
        "{text}"
    );
    let index = read(&f.root().join(".wiki/index.json"));
    assert!(index.contains(&format!("{shown}/tide.md")), "{index}");
    assert!(index.contains("\"memory\": true"), "{index}");
    assert!(f.claude_log().contains("--scope local"));
    assert!(out(&a).contains("memory folder"));
    f.setup(&["--remove", "--agent", "claude", "--scope", "local"])
        .success();
    assert!(note.is_file());
}

#[test]
fn setup_local_creates_the_empty_folder() {
    let f = committed();
    f.setup(&["--agent", "claude", "--scope", "local"])
        .success();
    assert!(f.local_memory().is_dir());
}

fn no_memory_section(f: &Fixture) {
    let text = read(&f.root().join(".agents/skills/wiki-navigation/SKILL.md"));
    assert!(
        !text.contains("Memory") && !text.contains("<memory-dir>"),
        "{text}"
    );
    assert!(!f.root().join(".agents/memory").exists());
}

#[test]
fn setup_local_without_a_git_repo_says_so_and_continues() {
    let f = fixture(true, true);
    real_git(&f);
    fs::remove_dir_all(f.root().join(".git")).unwrap();
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    assert!(
        out(&a).contains("no local memory: not a git repo"),
        "{}",
        out(&a)
    );
    no_memory_section(&f);
}

#[test]
fn setup_local_in_a_repo_with_no_commits_says_so_and_continues() {
    let f = fixture(true, true);
    real_git(&f);
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    assert!(
        out(&a).contains("no local memory: no commits yet"),
        "{}",
        out(&a)
    );
    no_memory_section(&f);
}

#[test]
fn setup_local_without_git_on_path_says_so() {
    let f = fixture(true, true);
    let a = f.setup(&["--agent", "pi", "--scope", "local"]).success();
    assert!(
        out(&a).contains("no local memory: git is not on PATH"),
        "{}",
        out(&a)
    );
    no_memory_section(&f);
}

#[test]
fn setup_local_without_a_data_dir_says_so() {
    let f = committed();
    let a = Command::cargo_bin("wiki")
        .unwrap()
        .args(["setup", "--agent", "pi", "--scope", "local"])
        .current_dir(f.root())
        .env("PATH", f.bin())
        .env_remove("HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("LOCALAPPDATA")
        .assert()
        .success();
    assert!(
        out(&a).contains("no local memory: no data dir"),
        "{}",
        out(&a)
    );
    no_memory_section(&f);
}

#[test]
fn setup_local_twice_reports_everything_already_there() {
    let f = committed();
    let args = ["--agent", "claude", "--scope", "local"];
    f.setup(&args).success();
    let o = out(&f.setup(&args).success());
    assert!(o.contains("already set up: memory folder"), "{o}");
    assert!(
        o.contains("already set up: .claude/skills/wiki-memory/SKILL.md"),
        "{o}"
    );
    assert!(!o.contains("skipped"), "{o}");
}

/// A `codex` that logs its argv and answers `mcp get wiki` as our server once `mcp add` ran.
fn codex_stub(f: &Fixture) {
    let t = f.tmp.path().display();
    f.stub(
        "codex",
        &format!(
            r#"echo "$@" >> {t}/codex.log
case "$1 $2" in
"mcp get") [ -f {t}/codex.state ] || {{ echo "Error: No MCP server named 'wiki' found." >&2; exit 1; }}
  printf 'wiki\n  enabled: true\n  transport: stdio\n  command: wiki\n  args: serve --source .\n' ;;
"mcp add") echo x > {t}/codex.state ;;
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
            "{{\n  \"mcpServers\": {{\n    \"wiki\": {}\n  }}\n}}\n",
            ENTRY.replace('\n', "\n    ")
        )
    );
    assert!(
        out(&a).contains("cursor-agent mcp enable wiki"),
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
        log.contains("mcp add wiki -- wiki serve --source ."),
        "{log}"
    );
    let block = format!("<!-- wiki:navigation -->\n{NAV}<!-- /wiki:navigation -->\n");
    assert_eq!(read(&agents), format!("# Team rules\n\n{block}"));

    let a = f.setup(&args).success();
    for l in out(&a).lines().filter(|l| !l.starts_with("Compiled")) {
        assert!(l.starts_with("already set up:"), "{l}");
    }

    f.setup(&["--remove", "--agent", "codex", "--scope", "repo"])
        .success();
    assert_eq!(read(&agents), "# Team rules\n");
    assert!(read(&f.tmp.path().join("codex.log")).contains("mcp remove wiki"));
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
        "case \"$1 $2\" in \"mcp get\") printf 'wiki\\n  command: other\\n' ;; esac",
    );
    let a = f.setup(&["--agent", "codex", "--scope", "repo"]).code(1);
    assert!(
        out(&a)
            .contains("skipped Codex wiki server (~/.codex/config.toml): another \"wiki\" server"),
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
        o.contains("skipped AGENTS.md wiki navigation steps: a new AGENTS.md would hide CLAUDE.md"),
        "{o}"
    );
    assert!(o.contains("wrote Codex wiki server"), "{o}");

    // With AGENTS.md already there, the steps go into it as usual.
    fs::write(f.root().join("AGENTS.md"), "# Agents\n").unwrap();
    f.setup(&["--agent", "codex", "--scope", "repo"]).success();
    assert!(read(&f.root().join("AGENTS.md")).contains("<!-- wiki:navigation -->"));
}
