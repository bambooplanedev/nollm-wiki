//! `install.sh` against a `file://` release in a temp dir, and once the real modules end to end.
mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::{
    assert_binary, kulman, per_target_kulman, release, snapshot, state, stderr, stdout,
    version_dir, KULMAN_BYTES,
};
use serde_json::{json, Value};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../install.sh");
const KALKA: &[u8] = b"#!/bin/sh\necho kalka\n";

/// A temp root holding `release/` (0.7.0: kalka, and kulman as `kulman_bytes` per target); the
/// home `install.sh` is pointed at is `<root>/home`.
fn setup(kulman_bytes: &[u8]) -> (tempfile::TempDir, String) {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("release");
    let base = release(&dir, "0.7.0", &[("kalka", KALKA)]);
    per_target_kulman(&dir, "0.7.0", kulman_bytes);
    (t, base)
}

/// `sh` reading `script` from stdin, with `KULMAN_HOME=home`, the release, and `HOME` a temp
/// dir beside `home`.
fn sh(home: &Path, base: &str, script: &[u8]) -> Output {
    let user = home.with_file_name("user-home");
    fs::create_dir_all(&user).unwrap();
    let mut child = Command::new("sh")
        .env("KULMAN_HOME", home)
        .env("KULMAN_RELEASE_URL", base)
        .env("HOME", &user)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(script).unwrap();
    child.wait_with_output().unwrap()
}

fn install_sh(home: &Path, base: &str) -> Output {
    sh(home, base, &fs::read(SCRIPT).unwrap())
}

/// Rewrites every `kulman-v…` line of the latest `SHA256SUMS` with `edit`.
fn edit_sums(t: &Path, edit: impl Fn(&str) -> String) {
    let path = t.join("release/latest/download/SHA256SUMS");
    let sums: String = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| {
            if l.contains("  kulman-v0.7.0-") {
                edit(l)
            } else {
                l.to_string()
            }
        })
        .map(|l| l + "\n")
        .collect();
    fs::write(path, sums).unwrap();
}

/// Paths under `dir`, relative.
fn paths(dir: &Path) -> Vec<PathBuf> {
    snapshot(dir).into_keys().collect()
}

/// Nothing was written to the user's home, but for `.cache/rosetta` (where Rosetta, emulating
/// `x86_64` in a Linux container, keeps its own cache).
fn nothing_in_user_home(t: &Path) {
    let written: Vec<PathBuf> = paths(&t.join("user-home"))
        .into_iter()
        .filter(|p| p != Path::new(".cache") && !p.starts_with(".cache/rosetta"))
        .collect();
    assert_eq!(written, Vec::<PathBuf>::new());
}

#[test]
fn install_sh_puts_kulman_in_bin() {
    let (t, base) = setup(KULMAN_BYTES);
    let home = t.path().join("home");
    let out = install_sh(&home, &base);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "kulman", KULMAN_BYTES);
    // Nothing else is written: no state, no temp dir, nothing in the user's home.
    assert_eq!(paths(&home), [PathBuf::from("bin"), "bin/kulman".into()]);
    nothing_in_user_home(t.path());
    let bin = home.join("bin");
    assert_eq!(
        stdout(&out),
        format!(
            "kulman installed in {}\nnext: {}/kulman install\n",
            bin.display(),
            bin.display()
        )
    );
    // Again over the first: replaced in place.
    let out = install_sh(&home, &base);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(paths(&home), [PathBuf::from("bin"), "bin/kulman".into()]);
}

#[test]
fn install_sh_defaults_to_dot_kulman_home() {
    let (t, base) = setup(KULMAN_BYTES);
    let user = t.path().join("user-home");
    fs::create_dir_all(&user).unwrap();
    let out = Command::new("sh")
        .arg(SCRIPT)
        .env_remove("KULMAN_HOME")
        .env("KULMAN_RELEASE_URL", &base)
        .env("HOME", &user)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&user.join(".kulman-home"), "kulman", KULMAN_BYTES);
    assert!(!user.join(".kulman").exists());
}

#[test]
fn install_sh_over_an_installation_says_update() {
    let (t, base) = setup(KULMAN_BYTES);
    let home = t.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("state.json"),
        r#"{"version":"0.6.0","modules":[]}"#,
    )
    .unwrap();
    let out = install_sh(&home, &base);
    assert!(out.status.success(), "{}", stderr(&out));
    let bin = home.join("bin");
    assert_eq!(
        stdout(&out),
        format!(
            "kulman installed in {}\nkulman replaced; run \"{}/kulman update\" to bring the \
             modules to the same version\n",
            bin.display(),
            bin.display()
        )
    );
}

#[test]
fn install_sh_reads_a_crlf_sha256sums() {
    let (t, base) = setup(KULMAN_BYTES);
    let path = t.path().join("release/latest/download/SHA256SUMS");
    let crlf = fs::read_to_string(&path).unwrap().replace('\n', "\r\n");
    fs::write(&path, crlf).unwrap();
    let home = t.path().join("home");
    let out = install_sh(&home, &base);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "kulman", KULMAN_BYTES);
}

#[test]
fn install_sh_leaves_nothing_when_the_archive_is_missing() {
    let (t, base) = setup(KULMAN_BYTES);
    for target in common::TARGETS {
        let name = format!("kulman-v0.7.0-{target}.tar.gz");
        fs::remove_file(version_dir(&t.path().join("release"), "0.7.0").join(name)).unwrap();
    }
    let home = t.path().join("home");
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("cannot download"), "{}", stderr(&out));
    assert!(!home.exists());
    fs::create_dir_all(&home).unwrap();
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(paths(&home), Vec::<PathBuf>::new());
}

#[test]
fn install_sh_rejects_a_bad_checksum() {
    let (t, base) = setup(KULMAN_BYTES);
    edit_sums(t.path(), |l| format!("{}{}", "0".repeat(64), &l[64..]));
    let home = t.path().join("home");
    // A home this run created is removed again.
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("checksum mismatch"),
        "{}",
        stderr(&out)
    );
    assert!(!home.exists());
    // A home that existed keeps what it held, and gains nothing.
    fs::create_dir_all(home.join("bin")).unwrap();
    fs::write(home.join("bin/kulman"), "old").unwrap();
    let before = snapshot(&home);
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(snapshot(&home), before);
}

#[test]
fn install_sh_needs_exactly_one_line_for_this_machine() {
    for (edit, expect) in [
        (
            (|l: &str| format!("{l}\n{l}")) as fn(&str) -> String,
            "2 lines",
        ),
        (|_: &str| String::new(), "0 lines"),
        // A version that is not X.Y.Z, or a short hash: not a kulman line at all.
        (|l: &str| l.replace("v0.7.0", "v0.7"), "0 lines"),
        (|l: &str| l[1..].to_string(), "bad line"),
    ] {
        let (t, base) = setup(KULMAN_BYTES);
        edit_sums(t.path(), edit);
        let home = t.path().join("home");
        let out = install_sh(&home, &base);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        assert!(stderr(&out).contains(expect), "{expect}: {}", stderr(&out));
        assert!(!home.exists());
    }
}

#[test]
fn install_sh_refuses_a_bin_it_cannot_use() {
    let (t, base) = setup(KULMAN_BYTES);
    let home = t.path().join("home");
    let elsewhere = t.path().join("elsewhere");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, home.join("bin")).unwrap();
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let want = format!(
        "{} is not a directory kulman can use",
        home.join("bin").display()
    );
    assert!(stderr(&out).contains(&want), "{}", stderr(&out));
    assert_eq!(paths(&elsewhere), Vec::<PathBuf>::new());
    // bin/kulman a directory: mv would move the binary into it.
    fs::remove_file(home.join("bin")).unwrap();
    fs::create_dir_all(home.join("bin/kulman")).unwrap();
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(paths(&home), [PathBuf::from("bin"), "bin/kulman".into()]);
}

#[test]
fn install_sh_takes_only_a_regular_file() {
    let (t, base) = setup(KULMAN_BYTES);
    // The archive for every target holds `kulman` as a symlink.
    let stage = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/bin/sh", stage.path().join("kulman")).unwrap();
    let vdir = version_dir(&t.path().join("release"), "0.7.0");
    for target in common::TARGETS {
        let file = vdir.join(format!("kulman-v0.7.0-{target}.tar.gz"));
        common::pack(stage.path(), &["kulman"], &file);
        let sum = common::sha256(&file);
        let name = format!("kulman-v0.7.0-{target}.tar.gz");
        edit_sums(t.path(), |l| {
            if l.ends_with(&name) {
                format!("{sum}  {name}")
            } else {
                l.to_string()
            }
        });
    }
    let home = t.path().join("home");
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("no regular file"), "{}", stderr(&out));
    assert!(!home.exists());
}

#[test]
fn install_sh_takes_only_https_or_file() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    for base in [
        "http://example.com/releases",
        "ftp://x",
        "https://",
        "file://x/y",
    ] {
        let out = install_sh(&home, base);
        assert_eq!(out.status.code(), Some(2), "{base}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("must start with https://"),
            "{}",
            stderr(&out)
        );
    }
    assert!(!home.exists());
}

#[test]
fn install_sh_then_kulman_install_works() {
    let real = fs::read(env!("CARGO_BIN_EXE_kulman")).unwrap();
    let (t, base) = setup(&real);
    let home = t.path().join("home");
    let out = install_sh(&home, &base);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "kulman", &real);
    let out = Command::new(home.join("bin/kulman"))
        .args(["install", "--yes"])
        .env("KULMAN_HOME", &home)
        .env("KULMAN_RELEASE_URL", &base)
        .env("HOME", t.path().join("user-home"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "kalka", KALKA);
    assert_binary(&home, "kulman", &real);
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["kalka"]})
    );
}

#[test]
fn a_truncated_script_does_nothing() {
    let (t, base) = setup(KULMAN_BYTES);
    let home = t.path().join("home");
    let script = fs::read(SCRIPT).unwrap();
    let call = script.len() - b"main \"$@\"\n".len();
    assert!(script[call..].starts_with(b"main "));
    // Eighths of the script, the cut just before the call, and one inside it. A prefix may exit
    // 0 (nothing left to run) or not; either way nothing is created.
    let cuts = (1..8).map(|i| script.len() * i / 8).chain([call, call + 6]);
    for cut in cuts {
        sh(&home, &base, &script[..cut]);
        assert!(!home.exists(), "cut at {cut}");
        nothing_in_user_home(t.path());
    }
}

/// A dir holding `git` and `tar` only, as scripts that run the real ones: a `PATH` with no
/// `claude` or `codex`.
fn git_and_tar_only(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("path");
    fs::create_dir_all(&bin).unwrap();
    for tool in ["git", "tar"] {
        let found = Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        let real = String::from_utf8(found.stdout).unwrap();
        assert!(found.status.success(), "{tool} is not on PATH");
        let path = bin.join(tool);
        fs::write(&path, format!("#!/bin/sh\nexec '{}' \"$@\"\n", real.trim())).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

#[test]
#[ignore = "needs target/release/kalka and target/release/plaz (cargo build --release)"]
fn setup_with_the_real_kalka_and_plaz() {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    let built = |name: &str| fs::read(target.join("release").join(name)).ok();
    let (Some(kalka), Some(plaz)) = (built("kalka"), built("plaz")) else {
        eprintln!("skipped: no {}/release/{{kalka,plaz}}", target.display());
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("kalka", &kalka), ("plaz", &plaz)],
    );
    let home = t.path().join("home");
    let out = kulman(&home, &base)
        .args(["install", "--modules", "kalka,plaz", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));

    let project = t.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("src/lib.rs"),
        "pub fn sign(x: i32) -> i32 {\n    if x < 0 { -1 } else { 1 }\n}\n",
    )
    .unwrap();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&project)
        .status()
        .unwrap();
    assert!(git.success());
    let path = git_and_tar_only(t.path());
    let setup = |extra: &[&str]| {
        let out = kulman(&home, &base)
            .current_dir(&project)
            .env("PATH", &path)
            .args(["setup", "--agent", "cursor", "--scope", "repo"])
            .args(extra)
            .arg("--yes")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}\n{}", stdout(&out), stderr(&out));
    };
    setup(&["--no-memory"]);
    assert!(project.join(".kulman/map").is_dir());
    assert!(!project.join(".agents/memory").exists());
    let mcp = || -> Value {
        fs::read(project.join(".cursor/mcp.json"))
            .map_or(Value::Null, |b| serde_json::from_slice(&b).unwrap())
    };
    assert!(mcp()["mcpServers"]["kalka"].is_object(), "{}", mcp());
    setup(&["--remove"]);
    assert!(mcp()["mcpServers"].get("kalka").is_none(), "{}", mcp());
}

#[test]
fn install_sh_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(SCRIPT).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "{mode:o}");
}
