mod common;

use std::fs;
use std::path::Path;

use common::{
    archive_path, assert_binary, edit_manifest, kulman, no_tmp_left, pack, read_manifest, release,
    replace_archive, set_sha256, snapshot, state, stderr, stdout, version_dir, KULMAN_BYTES,
};
use serde_json::{json, Value};

const KALKA: &[u8] = b"#!/bin/sh\necho kalka\n";
const PLAZ: &[u8] = b"#!/bin/sh\necho plaz\n";

/// A temp root holding `release/` (version 0.7.0, kalka and plaz) and the path `home`.
fn setup() -> (tempfile::TempDir, String) {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("kalka", KALKA), ("plaz", PLAZ)],
    );
    (t, base)
}

/// Installs kalka, then runs `install --modules plaz` after `break_it`; asserts exit 1, the
/// stderr fragment, and that nothing under the temp root (home included) changed. Returns stderr.
fn plaz_fails_and_changes_nothing(break_it: impl FnOnce(&Path), expect: &str) -> String {
    let (t, base) = setup();
    let home = t.path().join("home");
    kulman(&home, &base)
        .args(["install", "--modules", "kalka"])
        .assert()
        .success();
    break_it(&t.path().join("release"));
    let before = snapshot(t.path());
    let out = kulman(&home, &base)
        .args(["install", "--modules", "plaz"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains(expect), "{}", stderr(&out));
    assert_eq!(snapshot(t.path()), before);
    stderr(&out)
}

#[test]
fn install_defaults_installs_every_default_module() {
    let (t, base) = setup();
    let home = t.path().join("home");
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "kalka", KALKA);
    assert_binary(&home, "plaz", PLAZ);
    assert_binary(&home, "kulman", KULMAN_BYTES);
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["kalka", "plaz"]})
    );
    assert_eq!(
        fs::read(home.join("manifest.json")).unwrap(),
        fs::read(version_dir(&t.path().join("release"), "0.7.0").join("manifest.json")).unwrap()
    );
    let text = stdout(&out);
    for line in [
        "installed kalka 0.7.0",
        "installed plaz 0.7.0",
        "installed kulman 0.7.0",
    ] {
        assert!(text.lines().any(|l| l == line), "{text}");
    }
    no_tmp_left(&home);
}

#[test]
fn install_modules_takes_only_the_named() {
    let (t, base) = setup();
    let home = t.path().join("home");
    kulman(&home, &base)
        .args(["install", "--modules", "kalka", "--yes"])
        .assert()
        .success();
    assert_binary(&home, "kalka", KALKA);
    assert!(!home.join("bin/plaz").exists());
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["kalka"]})
    );

    // The same version again adds the missing module; a stale temp entry is cleared first.
    fs::create_dir(home.join(".tmp-stale")).unwrap();
    fs::write(home.join(".tmp-stale/junk"), "x").unwrap();
    kulman(&home, &base)
        .args(["install", "--modules", "plaz"])
        .assert()
        .success();
    assert_binary(&home, "plaz", PLAZ);
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["kalka", "plaz"]})
    );
    no_tmp_left(&home);
}

#[test]
fn install_a_module_whose_default_is_false_is_left_out() {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("kalka", KALKA), ("plaz", PLAZ), ("extra", b"extra")],
    );
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    assert!(!home.join("bin/extra").exists());
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["kalka", "plaz"]})
    );
}

#[test]
fn install_unknown_module_is_usage() {
    let (t, base) = setup();
    let home = t.path().join("home");
    let out = kulman(&home, &base)
        .args(["install", "--modules", "kalka,nope"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("nope") && err.contains("kalka, plaz"), "{err}");
    assert!(
        !err.contains("kulman,") && !err.contains(", kulman"),
        "{err}"
    );
    assert!(!home.exists());
}

#[test]
fn install_checksum_mismatch_changes_nothing() {
    plaz_fails_and_changes_nothing(
        |r| set_sha256(r, "0.7.0", "plaz", &"0".repeat(64)),
        "kulman: plaz: checksum mismatch",
    );
}

#[test]
fn install_missing_archive_changes_nothing() {
    plaz_fails_and_changes_nothing(
        |r| fs::remove_file(archive_path(r, "0.7.0", "plaz")).unwrap(),
        "kulman: plaz: download failed",
    );
}

#[test]
fn install_broken_archive_changes_nothing() {
    plaz_fails_and_changes_nothing(
        |r| replace_archive(r, "0.7.0", "plaz", b"this is not a tar.gz"),
        "kulman: plaz: cannot unpack: ",
    );
}

/// Packs `build`'s contents (member `plaz` among them) as plaz's archive.
fn plaz_archive(release: &Path, members: &[&str], build: impl FnOnce(&Path)) {
    let stage = tempfile::tempdir().unwrap();
    build(stage.path());
    let out = stage.path().join("out.tar.gz");
    pack(stage.path(), members, &out);
    replace_archive(release, "0.7.0", "plaz", &fs::read(&out).unwrap());
}

#[test]
fn install_refuses_a_symlink_member() {
    plaz_fails_and_changes_nothing(
        |r| {
            plaz_archive(r, &["plaz"], |d| {
                std::os::unix::fs::symlink("../../outside", d.join("plaz")).unwrap();
            });
        },
        "kulman: plaz: archive holds no regular file plaz",
    );
}

#[test]
fn install_refuses_a_directory_member() {
    plaz_fails_and_changes_nothing(
        |r| {
            plaz_archive(r, &["plaz"], |d| {
                fs::create_dir(d.join("plaz")).unwrap();
                fs::write(d.join("plaz/inner"), "x").unwrap();
            });
        },
        "kulman: plaz: archive holds no regular file plaz",
    );
}

#[test]
fn install_refuses_a_hard_link_member() {
    // `plaz` is stored as a hard link to `x`, and only `plaz` is extracted. Both tars refuse a
    // link to a member they did not extract; were one to make a file, its second link (or
    // its content) would still keep it out — the `nlink == 1` check has its own unit test.
    let err = plaz_fails_and_changes_nothing(
        |r| {
            plaz_archive(r, &["x", "plaz"], |d| {
                fs::write(d.join("x"), PLAZ).unwrap();
                fs::hard_link(d.join("x"), d.join("plaz")).unwrap();
            });
        },
        "kulman: plaz: ",
    );
    let line = err.lines().next().unwrap();
    assert!(
        line.starts_with("kulman: plaz: cannot unpack: ")
            || line == "kulman: plaz: archive holds no regular file plaz",
        "{err}"
    );
}

#[test]
fn install_refuses_a_member_written_through_a_symlink() {
    // `plaz` is a symlink to a directory outside home, then `plaz/evil` is a file: tar must
    // not write it there (the snapshot covers the outside directory).
    plaz_fails_and_changes_nothing(
        |r| {
            let dir = r.with_file_name("outside");
            fs::create_dir(&dir).unwrap();
            plaz_archive(r, &["plaz", "plaz/evil"], |d| {
                std::os::unix::fs::symlink(&dir, d.join("plaz")).unwrap();
                fs::write(dir.join("evil"), "x").unwrap();
            });
            fs::remove_file(dir.join("evil")).unwrap();
        },
        "kulman: plaz: ",
    );
}

#[test]
fn install_refuses_bad_entries_in_the_manifest() {
    type Edit = fn(&mut Value);
    let bad: [(Edit, &str); 4] = [
        (
            |m| m["modules"][1]["requires"] = json!(["../y"]),
            "kulman: bad name in manifest: ../y",
        ),
        (
            |m| {
                for a in m["kulman"]["archives"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                {
                    a["file"] = json!("../k.tar.gz");
                }
            },
            "kulman: bad name in manifest: ../k.tar.gz",
        ),
        (
            |m| m["modules"][1]["name"] = json!("kulman"),
            "kulman: bad name in manifest: kulman",
        ),
        (
            |m| {
                for a in m["modules"][1]["archives"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                {
                    a["sha256"] = json!("abc");
                }
            },
            "kulman: bad sha256 in manifest: abc",
        ),
    ];
    for (edit, expect) in bad {
        plaz_fails_and_changes_nothing(|r| edit_manifest(r, "0.7.0", edit), expect);
    }
}

#[test]
fn install_bounds_what_it_downloads() {
    // The manifest's limit end to end. The archive's (256 MiB, too big to write on every
    // run) goes through the same `download`, whose unit test covers the limit.
    let err = plaz_fails_and_changes_nothing(
        |r| {
            edit_manifest(r, "0.7.0", |m| {
                m["padding"] = json!(" ".repeat(1 << 20));
            });
        },
        "kulman: manifest.json: download failed: larger than 1 MiB",
    );
    assert_eq!(err.lines().count(), 1, "{err}");
}

#[test]
fn install_refuses_a_path_like_name() {
    plaz_fails_and_changes_nothing(
        |r| {
            edit_manifest(r, "0.7.0", |m| {
                m["modules"][1]["name"] = json!("../x");
            });
        },
        "kulman: bad name in manifest: ../x",
    );
    plaz_fails_and_changes_nothing(
        |r| {
            edit_manifest(r, "0.7.0", |m| {
                for a in m["modules"][1]["archives"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                {
                    a["file"] = json!("../plaz.tar.gz");
                }
            });
        },
        "kulman: bad name in manifest: ../plaz.tar.gz",
    );
    let (t, base) = setup();
    let home = t.path().join("home");
    let out = kulman(&home, &base)
        .args(["install", "--modules", "../x"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("kulman: bad name in --modules: ../x"));
    assert!(!home.exists());
}

#[test]
fn install_refuses_a_malformed_state() {
    for (bad, expect) in [
        (&b"{not json"[..], "state.json"),
        (
            &br#"{"version":"0.7.0","modules":["../../victim"]}"#[..],
            "kulman: bad name in state.json: ../../victim",
        ),
    ] {
        let (t, base) = setup();
        let home = t.path().join("home");
        kulman(&home, &base)
            .args(["install", "--modules", "kalka"])
            .assert()
            .success();
        fs::write(home.join("state.json"), bad).unwrap();
        let before = snapshot(t.path());
        let out = kulman(&home, &base)
            .args(["install", "--modules", "plaz"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        assert!(stderr(&out).contains(expect), "{}", stderr(&out));
        assert_eq!(snapshot(t.path()), before);
    }
}

#[test]
fn install_at_an_older_version_says_update() {
    let (t, base) = setup();
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    let newer = release(&t.path().join("newer"), "0.8.0", &[("kalka", KALKA)]);
    let before = snapshot(&home);
    let out = kulman(&home, &newer).arg("install").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("kulman: version 0.7.0 is installed; run kulman update"));
    assert_eq!(snapshot(&home), before);
}

#[test]
fn install_at_a_newer_version_says_remove() {
    let (t, base) = setup();
    let home = t.path().join("home");
    let newer = release(&t.path().join("newer"), "0.10.0", &[("kalka", KALKA)]);
    kulman(&home, &newer).arg("install").assert().success();
    let before = snapshot(&home);
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains(
        "kulman: version 0.10.0 is installed, newer than 0.7.0; \
         run kulman remove, then kulman install --version 0.7.0"
    ));
    assert_eq!(snapshot(&home), before);
}

#[test]
fn install_version_must_match_the_manifest() {
    let (t, base) = setup();
    let rel = t.path().join("release");
    let home = t.path().join("home");
    // download/v0.6.0/manifest.json that states 0.7.0.
    let v6 = version_dir(&rel, "0.6.0");
    fs::create_dir_all(&v6).unwrap();
    fs::write(
        v6.join("manifest.json"),
        serde_json::to_vec(&read_manifest(&rel, "0.7.0")).unwrap(),
    )
    .unwrap();
    let out = kulman(&home, &base)
        .args(["install", "--version", "0.6.0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("kulman: manifest.json of 0.6.0 states version 0.7.0"),
        "{}",
        stderr(&out)
    );
    assert!(!home.exists());

    let out = kulman(&home, &base)
        .args(["install", "--version", "v0.7"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));

    // The latest manifest is not read with --version: break it, install still works.
    fs::write(rel.join("latest/download/manifest.json"), "broken").unwrap();
    kulman(&home, &base)
        .args(["install", "--version", "0.7.0"])
        .assert()
        .success();
    assert_eq!(state(&home)["version"], "0.7.0");
}

#[test]
fn install_without_home_says_so() {
    let (t, base) = setup();
    let home = t.path().join("home");
    for kulman_home in [None, Some("")] {
        let mut cmd = kulman(&home, &base);
        cmd.env_remove("HOME").env_remove("KULMAN_HOME");
        if let Some(v) = kulman_home {
            cmd.env("KULMAN_HOME", v);
        }
        let out = cmd.arg("install").output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("kulman: set HOME or KULMAN_HOME"));
    }
    assert!(!home.exists());
}

#[test]
fn install_falls_back_to_dot_kulman_in_home() {
    let (t, base) = setup();
    let home = t.path().join("home");
    let user = t.path().join("user-home");
    kulman(&home, &base)
        .env_remove("KULMAN_HOME")
        .args(["install", "--modules", "kalka"])
        .assert()
        .success();
    assert_binary(&user.join(".kulman"), "kalka", KALKA);
    assert!(!home.exists());
}

#[test]
fn install_rejects_a_plain_http_base() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    for base in [
        "http://example.invalid/releases",
        "file://host/x",
        "ftp://x",
        "https://",
        "file:///",
        "https://example.invalid/releases?x=1",
        "https://example.invalid/releases#x",
    ] {
        let out = kulman(&home, base).arg("install").output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{base}");
    }
    assert!(!home.exists());
}

#[test]
fn install_without_tar_says_so() {
    let (t, base) = setup();
    let home = t.path().join("home");
    let empty = t.path().join("empty-path");
    fs::create_dir(&empty).unwrap();
    let out = kulman(&home, &base)
        .env("PATH", &empty)
        .arg("install")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("kulman: tar is required"),
        "{}",
        stderr(&out)
    );
    assert!(!home.exists());
}

#[test]
fn install_says_how_to_add_to_path() {
    let (t, base) = setup();
    let home = t.path().join("home");
    let bin = home.join("bin");
    let hint = format!("add to PATH: {}", bin.display());
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert!(stdout(&out).lines().any(|l| l == hint), "{}", stdout(&out));

    let path = std::env::join_paths(
        std::iter::once(bin.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let out = kulman(&home, &base)
        .env("PATH", path)
        .arg("install")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("add to PATH"), "{}", stdout(&out));
}

#[test]
fn a_failed_first_install_leaves_no_home() {
    let (t, base) = setup();
    set_sha256(&t.path().join("release"), "0.7.0", "plaz", &"0".repeat(64));
    let home = t.path().join("home");
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("kulman: plaz: checksum mismatch"));
    assert!(!home.exists());
}

#[test]
fn install_into_a_missing_parent_says_so() {
    let (t, base) = setup();
    let parent = t.path().join("missing");
    let out = kulman(&t.path().join("home"), &base)
        .env("KULMAN_HOME", parent.join("home"))
        .arg("install")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let expect = format!("kulman: {} does not exist", parent.display());
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert!(!parent.exists());
}

#[test]
fn a_run_waiting_on_a_home_that_vanishes_starts_again() {
    // This test holds the lock as a first install would, then removes home and the lock file
    // as that install does when it fails; the waiting run must create home again.
    let (t, base) = setup();
    let home = t.path().join("home");
    fs::create_dir(&home).unwrap();
    let lock = fs::File::create(home.join(".lock")).unwrap();
    lock.lock().unwrap();
    let user = t.path().join("user-home");
    fs::create_dir_all(&user).unwrap();
    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("kulman"))
        .env("KULMAN_HOME", &home)
        .env("KULMAN_RELEASE_URL", &base)
        .env("HOME", &user)
        .args(["install", "--modules", "kalka"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    fs::remove_file(home.join(".lock")).unwrap();
    fs::remove_dir(&home).unwrap();
    drop(lock);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "kalka", KALKA);
    assert!(home.join(".lock").exists());
}

#[test]
fn install_refuses_a_symlinked_bin() {
    let (t, base) = setup();
    let home = t.path().join("home");
    kulman(&home, &base)
        .args(["install", "--modules", "kalka"])
        .assert()
        .success();
    let outside = t.path().join("outside");
    fs::rename(home.join("bin"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, home.join("bin")).unwrap();
    let before = snapshot(t.path());
    let out = kulman(&home, &base)
        .args(["install", "--modules", "plaz"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!(
        "kulman: {} is not a directory kulman can use",
        home.join("bin").display()
    );
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn install_checks_every_build_before_any_download() {
    // kalka's archive is missing and plaz has no build at all: the error names plaz, so
    // nothing was fetched for kalka first.
    let (t, base) = setup();
    let rel = t.path().join("release");
    fs::remove_file(archive_path(&rel, "0.7.0", "kalka")).unwrap();
    edit_manifest(&rel, "0.7.0", |m| m["modules"][1]["archives"] = json!({}));
    let home = t.path().join("home");
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("kulman: plaz: no build for "),
        "{}",
        stderr(&out)
    );
    assert!(!home.exists());
}

#[test]
fn install_repairs_a_first_install_that_died_before_its_state() {
    let (t, base) = setup();
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    // Killed after manifest.json, before state.json: bin/ and the lock stay.
    fs::remove_file(home.join("state.json")).unwrap();
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["kalka", "plaz"]})
    );
    assert_binary(&home, "kalka", KALKA);
    assert_binary(&home, "plaz", PLAZ);
    assert_binary(&home, "kulman", KULMAN_BYTES);
    no_tmp_left(&home);
}
