//! `scripts/release-assemble.sh` over fake archives, and what it assembles read by `kulman` and
//! `install.sh` from a `file://` tree in a temp dir.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{assert_binary, kulman, pack, sha256, state, stderr, TARGETS};
use serde_json::json;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/release-assemble.sh"
);
const BINARIES: [&str; 3] = ["kalka", "plaz", "kulman"];
const VERSION: &str = "0.7.0";

/// This machine's target, as `kulman` and `install.sh` pick it.
const HERE: &str = if cfg!(target_os = "macos") {
    "aarch64-apple-darwin"
} else {
    "x86_64-unknown-linux-musl"
};

fn file(binary: &str, target: &str) -> String {
    format!("{binary}-v{VERSION}-{target}.tar.gz")
}

/// A fake binary: a script naming itself and its target.
fn bytes(binary: &str, target: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho {binary} {target}\n").into_bytes()
}

/// A temp root whose `assets/` holds the 6 archives.
fn assets() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("assets");
    fs::create_dir(&dir).unwrap();
    for binary in BINARIES {
        for target in TARGETS {
            pack_one(&dir, binary, target, &[binary]);
        }
    }
    (t, dir)
}

/// Packs `members` (each holding `binary`'s fake bytes) as `binary`'s archive for `target`.
fn pack_one(dir: &Path, binary: &str, target: &str, members: &[&str]) {
    let stage = tempfile::tempdir().unwrap();
    for m in members {
        let path = stage.path().join(m);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes(binary, target)).unwrap();
    }
    pack(stage.path(), members, &dir.join(file(binary, target)));
}

fn assemble(args: &[&Path]) -> Output {
    Command::new("sh").arg(SCRIPT).args(args).output().unwrap()
}

/// Assembles `assets/` and lays out `tree/`; returns the tree's `file://` base.
fn assembled(t: &Path, dir: &Path) -> String {
    let tree = t.join("tree");
    let out = assemble(&[Path::new(VERSION), dir, Path::new("--tree"), &tree]);
    assert!(out.status.success(), "{}", stderr(&out));
    format!("file://{}", tree.display())
}

fn fails(out: &Output, expect: &str) {
    assert_eq!(out.status.code(), Some(1), "{}", stderr(out));
    let err = stderr(out);
    assert!(err.contains(expect), "{err}");
}

#[test]
fn the_assembled_manifest_is_one_kulman_reads() {
    let (t, dir) = assets();
    let base = assembled(t.path(), &dir);
    let home = t.path().join("home");
    let out = kulman(&home, &base)
        .args(["install", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    for binary in BINARIES {
        assert_binary(&home, binary, &bytes(binary, HERE));
    }
    assert_eq!(
        state(&home),
        json!({"version": VERSION, "modules": ["kalka", "plaz"]})
    );
    // Both places of the tree hold the manifest written beside the archives.
    let manifest = fs::read(dir.join("manifest.json")).unwrap();
    for at in ["latest/download", "download/v0.7.0"] {
        assert_eq!(
            fs::read(t.path().join("tree").join(at).join("manifest.json")).unwrap(),
            manifest
        );
    }
    let m: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(m["modules"][0]["options"][0]["name"], "memory");
    assert_eq!(m["modules"][0]["options"][0]["default"], true);
    assert_eq!(
        m["kulman"]["archives"][HERE],
        json!({"file": file("kulman", HERE), "sha256": sha256(&dir.join(file("kulman", HERE)))})
    );
}

#[test]
fn sha256sums_lists_every_archive_once() {
    let (t, dir) = assets();
    assembled(t.path(), &dir);
    let sums = fs::read_to_string(dir.join("SHA256SUMS")).unwrap();
    let mut lines: Vec<&str> = sums.lines().collect();
    lines.sort_unstable();
    let mut expect: Vec<String> = BINARIES
        .iter()
        .flat_map(|b| TARGETS.iter().map(move |t| file(b, t)))
        .map(|f| format!("{}  {f}", sha256(&dir.join(&f))))
        .collect();
    expect.sort_unstable();
    assert_eq!(lines, expect);
    assert_eq!(
        fs::read(t.path().join("tree/latest/download/SHA256SUMS")).unwrap(),
        sums.as_bytes()
    );
}

#[test]
fn a_missing_archive_fails_the_assembly() {
    let (_t, dir) = assets();
    let gone = file("plaz", "aarch64-apple-darwin");
    fs::remove_file(dir.join(&gone)).unwrap();
    fails(&assemble(&[Path::new(VERSION), &dir]), &gone);
    assert!(!dir.join("manifest.json").exists());
    assert!(!dir.join("SHA256SUMS").exists());
}

#[test]
fn an_archive_with_the_wrong_member_fails_the_assembly() {
    let members: [&[&str]; 3] = [&["bin/kalka"], &["kalka", "other"], &["./kalka"]];
    for members in members {
        let (t, dir) = assets();
        pack_one(&dir, "kalka", HERE, members);
        let tree = t.path().join("tree");
        let out = assemble(&[Path::new(VERSION), &dir, Path::new("--tree"), &tree]);
        fails(&out, &file("kalka", HERE));
        assert!(!dir.join("manifest.json").exists(), "{members:?}");
        assert!(!tree.exists(), "{members:?}");
    }
}

#[test]
fn a_bad_version_fails_the_assembly() {
    let (_t, dir) = assets();
    for v in ["v0.7.0", "0.7", "01.2.3", "0.7.0.1", "", "0.7.0\n"] {
        fails(&assemble(&[Path::new(v), &dir]), "version");
        assert!(!dir.join("manifest.json").exists(), "{v:?}");
    }
}

#[test]
fn install_sh_installs_from_the_assembled_tree() {
    let (t, dir) = assets();
    let base = assembled(t.path(), &dir);
    let published = t.path().join("tree/latest/download/install.sh");
    let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../../install.sh");
    assert_eq!(fs::read(&published).unwrap(), fs::read(repo).unwrap());
    let home = t.path().join("home");
    let user = t.path().join("user-home");
    fs::create_dir_all(&user).unwrap();
    let out = Command::new("sh")
        .arg(&published)
        .env("KULMAN_HOME", &home)
        .env("KULMAN_RELEASE_URL", &base)
        .env("HOME", &user)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "kulman", &bytes("kulman", HERE));
}
