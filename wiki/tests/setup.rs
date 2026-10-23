//! `wiki setup` end to end, with a stub `claude` and a `PATH` of stub binaries only.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

const SKILL: &str = include_str!("../src/setup/SKILL.md");
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
            .assert()
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
        let t = f.tmp.path().display();
        f.stub(
            "claude",
            &format!(
                r#"echo "$@" >> {t}/claude.log
case "$1 $2" in
"mcp get") [ -f {t}/state ] || {{ echo 'No MCP server named "wiki".'; exit 1; }}
  printf 'wiki:\n  Scope: Local config (private to you in this project)\n  Status: ok\n  Type: stdio\n  Command: wiki\n  Args: serve --source .\n' ;;
"mcp add-json") echo x > {t}/state ;;
"mcp remove") /bin/rm -f {t}/state ;;
esac"#
            ),
        );
    }
    f
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

fn out(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(a.get_output().stdout.clone()).unwrap()
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
    let pi = read(&r.join(".pi/mcp.json"));
    assert!(pi.contains("\"exposure\": \"direct\""), "{pi}");
    assert_eq!(
        read(&r.join(".claude/skills/wiki-navigation/SKILL.md")),
        SKILL
    );
    assert_eq!(
        read(&r.join(".agents/skills/wiki-navigation/SKILL.md")),
        SKILL
    );
    assert!(read(&r.join(".gitignore")).lines().any(|l| l == ".wiki/"));
    assert!(r.join(".wiki/index.json").exists());
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
        read(&f.root().join(".claude/skills/wiki-navigation/SKILL.md")),
        SKILL
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
        "/.claude/skills/wiki-navigation/",
        "/.agents/skills/wiki-navigation/",
    ] {
        assert!(ex.lines().any(|x| x == l), "{l} missing in {ex}");
    }
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
    let args = ["--agent", "claude,pi", "--scope", "repo"];
    f.setup(&args).success();
    let a = f
        .setup(&["--remove", "--agent", "claude,pi", "--scope", "repo"])
        .success();
    let mut want = before;
    want.push((f.root().join(".gitignore"), b".wiki/\n".to_vec()));
    want.sort();
    assert_eq!(files(&f.root()), want);
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
fn setup_local_uses_the_binary_path_when_wiki_is_not_on_path() {
    let f = fixture(false, false);
    f.setup(&["--agent", "pi", "--scope", "local"]).success();
    let v: serde_json::Value = serde_json::from_str(&read(&f.root().join(".pi/mcp.json"))).unwrap();
    let c = v["mcpServers"]["wiki"]["command"].as_str().unwrap();
    assert!(Path::new(c).is_absolute() && c.ends_with("wiki"), "{c}");
}
