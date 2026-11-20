//! `scripts/release-assemble.sh` over fake archives, and what it assembles read by `nollm-wiki-install` and
//! `install.sh` from a `file://` tree in a temp dir.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{assert_binary, nollm_wiki_install, pack, sha256, state, stderr, TARGETS};
use serde_json::json;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/release-assemble.sh"
);
const BINARIES: [&str; 3] = ["wiki", "plaz", "nollm-wiki-install"];
const VERSION: &str = "0.7.0";

/// This machine's target, as `nollm-wiki-install` and `install.sh` pick it.
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
fn the_assembled_manifest_is_one_nollm_wiki_install_reads() {
    let (t, dir) = assets();
    let base = assembled(t.path(), &dir);
    let home = t.path().join("home");
    let out = nollm_wiki_install(&home, &base)
        .args(["install", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    for binary in BINARIES {
        assert_binary(&home, binary, &bytes(binary, HERE));
    }
    assert_eq!(
        state(&home),
        json!({"version": VERSION, "modules": ["plaz", "wiki"]})
    );
    let manifest = fs::read(dir.join("manifest.json")).unwrap();
    let m: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(m["modules"][0]["options"][0]["name"], "memory");
    assert_eq!(m["modules"][0]["options"][0]["default"], true);
    assert_eq!(
        m["nollm-wiki-install"]["archives"][HERE],
        json!({"file": file("nollm-wiki-install", HERE), "sha256": sha256(&dir.join(file("nollm-wiki-install", HERE)))})
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
}

#[test]
fn the_tree_holds_the_control_files_at_both_addresses() {
    let (t, dir) = assets();
    assembled(t.path(), &dir);
    let tree = t.path().join("tree");
    let names = |at: &str| {
        let mut v: Vec<String> = fs::read_dir(tree.join(at))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort_unstable();
        v
    };
    let mut versioned: Vec<String> = BINARIES
        .iter()
        .flat_map(|b| TARGETS.iter().map(move |t| file(b, t)))
        .chain(["SHA256SUMS", "install.sh", "manifest.json"].map(String::from))
        .collect();
    versioned.sort_unstable();
    assert_eq!(names("download/v0.7.0"), versioned);
    assert_eq!(
        names("latest/download"),
        ["SHA256SUMS", "install.sh", "manifest.json"]
    );
    for f in ["SHA256SUMS", "install.sh", "manifest.json"] {
        let want = fs::read(dir.join(f)).unwrap();
        for at in ["latest/download", "download/v0.7.0"] {
            assert_eq!(fs::read(tree.join(at).join(f)).unwrap(), want, "{at}/{f}");
        }
    }
}

#[test]
fn the_tree_carries_the_signatures_beside_what_they_sign() {
    let (t, dir) = assets();
    let sigs = ["SHA256SUMS.minisig", "manifest.json.minisig"];
    for f in sigs {
        fs::write(dir.join(f), format!("a signature of {f}\n")).unwrap();
    }
    assembled(t.path(), &dir);
    for at in ["latest/download", "download/v0.7.0"] {
        let at = t.path().join("tree").join(at);
        for f in sigs {
            assert_eq!(
                fs::read(at.join(f)).unwrap(),
                fs::read(dir.join(f)).unwrap(),
                "{}",
                at.join(f).display()
            );
        }
    }
}

#[test]
fn a_failing_tree_leaves_the_directory_without_the_control_files() {
    let (t, dir) = assets();
    // A tree below a regular file cannot be made.
    let blocker = t.path().join("blocker");
    fs::write(&blocker, "").unwrap();
    let out = assemble(&[
        Path::new(VERSION),
        &dir,
        Path::new("--tree"),
        &blocker.join("tree"),
    ]);
    assert!(!out.status.success());
    for f in ["SHA256SUMS", "manifest.json", "install.sh"] {
        assert!(!dir.join(f).exists(), "{f}");
    }
}

#[test]
fn a_read_only_directory_is_refused_before_the_tree_is_built() {
    use std::os::unix::fs::PermissionsExt;
    let (t, dir) = assets();
    let mode = |m| fs::set_permissions(&dir, fs::Permissions::from_mode(m)).unwrap();
    mode(0o555);
    if fs::write(dir.join("probe"), "").is_ok() {
        // root: permissions do not stop it, so there is no refusal to see here.
        fs::remove_file(dir.join("probe")).unwrap();
        mode(0o755);
        panic!("needs a user that cannot write to a mode 555 directory (not root)");
    }
    let tree = t.path().join("tree");
    let out = assemble(&[Path::new(VERSION), &dir, Path::new("--tree"), &tree]);
    mode(0o755);
    fails(&out, "is not writable");
    assert!(!tree.exists());
}

#[test]
fn cdpath_does_not_move_the_script_off_its_repository() {
    let (_t, dir) = assets();
    let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let out = Command::new("sh")
        .arg("scripts/release-assemble.sh")
        .args([Path::new(VERSION), &dir])
        .current_dir(repo)
        .env("CDPATH", repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(dir.join("manifest.json").exists());
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
    let members: [&[&str]; 3] = [&["bin/wiki"], &["wiki", "other"], &["./wiki"]];
    for members in members {
        let (t, dir) = assets();
        pack_one(&dir, "wiki", HERE, members);
        let tree = t.path().join("tree");
        let out = assemble(&[Path::new(VERSION), &dir, Path::new("--tree"), &tree]);
        fails(&out, &file("wiki", HERE));
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
        .env("NOLLM_WIKI_HOME", &home)
        .env("NOLLM_WIKI_RELEASE_URL", &base)
        .env("HOME", &user)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(
        &home,
        "nollm-wiki-install",
        &bytes("nollm-wiki-install", HERE),
    );
}
