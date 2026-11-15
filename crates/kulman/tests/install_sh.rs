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
const CALQUE: &[u8] = b"#!/bin/sh\necho calque\n";
const PUBLIC_KEY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../release/kulman.pub");
/// The notice `install.sh` prints to stderr when there is no `minisign`.
const NO_MINISIGN: &str = "install.sh: signature not checked (minisign not found); kulman checks \
                           it on every install and update\n";
/// What `install.sh` runs, linked from this `PATH`.
const TOOLS: [&str; 20] = [
    "sh",
    "uname",
    "curl",
    "tar",
    "gzip",
    "mktemp",
    "sha256sum",
    "shasum",
    "dirname",
    "mkdir",
    "rm",
    "rmdir",
    "sed",
    "grep",
    "tr",
    "wc",
    "chmod",
    "mv",
    "cat",
    "printf",
];

/// A temp root holding `release/` (0.7.0: calque, and kulman as `kulman_bytes` per target); the
/// home `install.sh` is pointed at is `<root>/home`.
fn setup(kulman_bytes: &[u8]) -> (tempfile::TempDir, String) {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("release");
    let base = release(&dir, "0.7.0", &[("calque", CALQUE)]);
    per_target_kulman(&dir, "0.7.0", kulman_bytes);
    (t, base)
}

/// `dir` holding a link to each of `TOOLS` this `PATH` has: a `PATH` with no `minisign`, though
/// this machine may have one, but for a fake one a test puts there.
fn tools(dir: &Path) -> PathBuf {
    if !dir.join("sh").exists() {
        fs::create_dir_all(dir).unwrap();
        for tool in TOOLS {
            let found = Command::new("sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .unwrap();
            let real = String::from_utf8(found.stdout).unwrap();
            // `printf` is a builtin in most shells; `sha256sum` or `shasum` may be missing.
            if found.status.success() && real.starts_with('/') {
                std::os::unix::fs::symlink(real.trim(), dir.join(tool)).unwrap();
            }
        }
    }
    dir.to_path_buf()
}

/// A fake `minisign` on the `PATH` of `sh`: it writes its arguments, one per line, to
/// `<t>/minisign.args`, and the file named after `-VHm` to `<t>/minisign.file`, and exits `code`.
fn fake_minisign(t: &Path, code: i32) {
    use std::os::unix::fs::PermissionsExt;
    let path = tools(&t.join("tools")).join("minisign");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >'{}'\ncat -- \"$2\" >'{}'\nexit {code}\n",
        t.join("minisign.args").display(),
        t.join("minisign.file").display()
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// `sh` reading `script` from stdin, with `KULMAN_HOME=home`, the release, `HOME` a temp dir
/// beside `home`, and `PATH` the `tools` beside it.
fn sh(home: &Path, base: &str, script: &[u8]) -> Output {
    let user = home.with_file_name("user-home");
    fs::create_dir_all(&user).unwrap();
    let mut child = Command::new("sh")
        .env("PATH", tools(&home.with_file_name("tools")))
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
    assert_eq!(stderr(&out), NO_MINISIGN);
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
    assert_eq!(stderr(&out), NO_MINISIGN);
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

/// The latest `SHA256SUMS` of the release under `t`, and where its signature goes.
fn sums_and_signature(t: &Path) -> (PathBuf, PathBuf) {
    let at = t.join("release/latest/download");
    (at.join("SHA256SUMS"), at.join("SHA256SUMS.minisig"))
}

#[test]
fn install_sh_checks_the_signature_with_minisign() {
    let (t, base) = setup(KULMAN_BYTES);
    let (sums, sig) = sums_and_signature(t.path());
    // CRLF: the file is verified as downloaded, not as read.
    let crlf = fs::read_to_string(&sums).unwrap().replace('\n', "\r\n");
    fs::write(&sums, &crlf).unwrap();
    fs::write(&sig, "a signature\n").unwrap();
    fake_minisign(t.path(), 0);
    let home = t.path().join("home");
    let out = install_sh(&home, &base);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "kulman", KULMAN_BYTES);
    assert_eq!(paths(&home), [PathBuf::from("bin"), "bin/kulman".into()]);
    let bin = home.join("bin");
    assert_eq!(
        stdout(&out),
        format!(
            "kulman installed in {}\nnext: {}/kulman install\n",
            bin.display(),
            bin.display()
        )
    );
    let args = fs::read_to_string(t.path().join("minisign.args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let [vm, got_sums, x, got_sig, p, key] = args[..] else {
        panic!("{args:?}");
    };
    assert_eq!((vm, x, p), ("-VHm", "-x", "-P"));
    let got_sums = Path::new(got_sums);
    assert_eq!(got_sums.parent().unwrap().parent().unwrap(), home);
    let tmp = got_sums.parent().unwrap().file_name().unwrap();
    assert!(
        tmp.to_str().unwrap().starts_with(".tmp-install."),
        "{tmp:?}"
    );
    assert_eq!(got_sums.file_name().unwrap(), "SHA256SUMS");
    assert_eq!(
        Path::new(got_sig),
        got_sums.with_file_name("SHA256SUMS.minisig")
    );
    assert_eq!(
        key,
        fs::read_to_string(PUBLIC_KEY)
            .unwrap()
            .lines()
            .nth(1)
            .unwrap()
    );
    assert_eq!(
        fs::read_to_string(t.path().join("minisign.file")).unwrap(),
        crlf
    );
}

#[test]
fn install_sh_stops_when_the_signature_does_not_verify() {
    let (t, base) = setup(KULMAN_BYTES);
    let (_, sig) = sums_and_signature(t.path());
    fs::write(&sig, "a signature\n").unwrap();
    fake_minisign(t.path(), 1);
    let home = t.path().join("home");
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "install.sh: SHA256SUMS: signature does not verify\n"
    );
    assert_eq!(stdout(&out), "");
    assert!(!home.exists());
    nothing_in_user_home(t.path());
    fs::create_dir_all(home.join("bin")).unwrap();
    fs::write(home.join("bin/kulman"), "old").unwrap();
    let before = snapshot(&home);
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(snapshot(&home), before);
}

#[test]
fn install_sh_takes_no_signature_past_4096_bytes() {
    let (t, base) = setup(KULMAN_BYTES);
    let (_, sig) = sums_and_signature(t.path());
    fs::write(&sig, "x".repeat(4097)).unwrap();
    fake_minisign(t.path(), 0);
    let home = t.path().join("home");
    let out = install_sh(&home, &base);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "install.sh: SHA256SUMS: signature does not verify\n"
    );
    assert!(!t.path().join("minisign.args").exists());
    assert!(!home.exists());
}

#[test]
fn install_sh_goes_on_in_a_local_tree_without_a_signature() {
    let (t, base) = setup(KULMAN_BYTES);
    fake_minisign(t.path(), 1);
    let home = t.path().join("home");
    let out = install_sh(&home, &base);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "kulman", KULMAN_BYTES);
    let bin = home.join("bin");
    assert_eq!(
        stderr(&out),
        "install.sh: signature not checked: local release tree\n"
    );
    assert_eq!(
        stdout(&out),
        format!(
            "kulman installed in {}\n\
             next: {}/kulman install\n",
            bin.display(),
            bin.display()
        )
    );
    assert!(!t.path().join("minisign.args").exists());
}

#[test]
fn install_sh_holds_the_release_key() {
    let key = fs::read_to_string(PUBLIC_KEY).unwrap();
    let key = key.lines().nth(1).unwrap();
    let script = fs::read_to_string(SCRIPT).unwrap();
    let held: Vec<&str> = script
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("ki_key="))
        .collect();
    assert_eq!(held, [key]);
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
    assert_binary(&home, "calque", CALQUE);
    assert_binary(&home, "kulman", &real);
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["calque"]})
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
#[ignore = "needs target/release/calque and target/release/plaz (cargo build --release)"]
fn setup_with_the_real_calque_and_plaz() {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    let built = |name: &str| fs::read(target.join("release").join(name)).ok();
    let (Some(calque), Some(plaz)) = (built("calque"), built("plaz")) else {
        panic!(
            "no {}/release/{{calque,plaz}}: run cargo build --release",
            target.display()
        );
    };
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("calque", &calque), ("plaz", &plaz)],
    );
    let home = t.path().join("home");
    let out = kulman(&home, &base)
        .args(["install", "--modules", "calque,plaz", "--yes"])
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
    assert!(mcp()["mcpServers"]["calque"].is_object(), "{}", mcp());
    setup(&["--remove"]);
    assert!(mcp()["mcpServers"].get("calque").is_none(), "{}", mcp());

    // No agent in the project and none named: the wiki and the map all the same.
    assert!(!project.join(".cursor").exists());
    fs::remove_dir_all(project.join(".kulman")).unwrap();
    let out = kulman(&home, &base)
        .current_dir(&project)
        .env("PATH", &path)
        .args(["setup", "--scope", "repo", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}\n{}", stdout(&out), stderr(&out));
    assert!(project.join(".kulman/index.md").is_file());
    assert!(project.join(".kulman/map").is_dir());
    assert!(stderr(&out).contains("no agent set up"), "{}", stderr(&out));
}

#[test]
fn install_sh_does_not_look_for_glibc() {
    let script = fs::read_to_string(SCRIPT).unwrap();
    assert!(!script.contains("ld-linux"));
    assert!(!script.contains("libc.so"));
}

#[test]
fn install_sh_has_no_build_for_an_intel_mac() {
    use std::os::unix::fs::PermissionsExt;
    let (t, base) = setup(KULMAN_BYTES);
    let home = t.path().join("home");
    let user = t.path().join("user-home");
    fs::create_dir_all(&user).unwrap();
    let bin = t.path().join("path");
    fs::create_dir_all(&bin).unwrap();
    let uname = bin.join("uname");
    fs::write(
        &uname,
        "#!/bin/sh\ncase $1 in -s) echo Darwin ;; -m) echo x86_64 ;; esac\n",
    )
    .unwrap();
    fs::set_permissions(&uname, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = Command::new("sh")
        .arg(SCRIPT)
        .env("PATH", path)
        .env("KULMAN_HOME", &home)
        .env("KULMAN_RELEASE_URL", &base)
        .env("HOME", &user)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no build for Darwin/x86_64"),
        "{}",
        stderr(&out)
    );
    assert!(!home.exists());
    nothing_in_user_home(t.path());
}
