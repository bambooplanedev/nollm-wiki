mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{
    assert_binary, kulman, no_tmp_left, release, set_sha256, snapshot, state, stderr, stdout,
    version_dir, KULMAN_BYTES,
};
use serde_json::json;

const KALKA: &[u8] = b"#!/bin/sh\necho kalka\n";
const PLAZ: &[u8] = b"#!/bin/sh\necho plaz\n";
const KALKA_8: &[u8] = b"#!/bin/sh\necho kalka 0.8\n";
const PLAZ_8: &[u8] = b"#!/bin/sh\necho plaz 0.8\n";

/// A temp root with `release/` at 0.7.0 (kalka and plaz) and `home` holding both.
fn installed() -> (tempfile::TempDir, String, std::path::PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("kalka", KALKA), ("plaz", PLAZ)],
    );
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    (t, base, home)
}

/// Publishes 0.8.0 (kalka and plaz, new bytes) as the latest in the same release dir.
fn publish_0_8(t: &Path) {
    release(
        &t.join("release"),
        "0.8.0",
        &[("kalka", KALKA_8), ("plaz", PLAZ_8)],
    );
}

fn lines(out: &std::process::Output) -> Vec<String> {
    stdout(out).lines().map(str::to_string).collect()
}

#[test]
fn status_lists_what_is_installed() {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("kalka", KALKA), ("plaz", PLAZ)],
    );
    let home = t.path().join("home");
    let out = kulman(&home, &base).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["nothing installed; run kulman install"]);
    assert!(!home.exists());

    kulman(&home, &base).arg("install").assert().success();
    let before = snapshot(t.path());
    let out = kulman(&home, &base).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let first = format!("kulman 0.7.0 in {}", home.display());
    assert_eq!(lines(&out), [first.as_str(), "kalka", "plaz"]);
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn status_announces_a_newer_release() {
    let (t, base, home) = installed();
    // 0.10.0 is newer than 0.7.0 only when the three parts are compared as integers.
    release(&t.path().join("release"), "0.10.0", &[("kalka", KALKA)]);
    let out = kulman(&home, &base).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out).last().unwrap(),
        "newer release: 0.10.0 (kulman update)"
    );
}

#[test]
fn status_survives_an_unreachable_release() {
    let (t, _, home) = installed();
    let missing = format!("file://{}", t.path().join("nowhere").display());
    let before = snapshot(t.path());
    for base in [missing.as_str(), "http://example.invalid/releases"] {
        let out = kulman(&home, base).output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        let text = lines(&out);
        assert_eq!(text.len(), 4, "{text:?}");
        assert!(
            text[3].starts_with("could not check for a newer release: "),
            "{text:?}"
        );
    }
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn update_moves_every_module() {
    let (t, base, home) = installed();
    // kulman's own binary is the same in both releases: mark the old one to see it replaced.
    fs::write(home.join("bin/kulman"), "old").unwrap();
    publish_0_8(t.path());
    let out = kulman(&home, &base)
        .args(["update", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "updated kalka 0.8.0",
            "updated plaz 0.8.0",
            "updated kulman 0.8.0"
        ]
    );
    assert_eq!(
        state(&home),
        json!({"version": "0.8.0", "modules": ["kalka", "plaz"]})
    );
    assert_binary(&home, "kalka", KALKA_8);
    assert_binary(&home, "plaz", PLAZ_8);
    assert_binary(&home, "kulman", KULMAN_BYTES);
    assert_eq!(
        fs::read(home.join("manifest.json")).unwrap(),
        fs::read(version_dir(&t.path().join("release"), "0.8.0").join("manifest.json")).unwrap()
    );
    no_tmp_left(&home);
}

#[test]
fn update_with_one_bad_archive_changes_nothing() {
    let (t, base, home) = installed();
    publish_0_8(t.path());
    set_sha256(&t.path().join("release"), "0.8.0", "plaz", &"0".repeat(64));
    let before = snapshot(t.path());
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("kulman: plaz: checksum mismatch"));
    assert_eq!(snapshot(t.path()), before);

    // A module the latest release no longer holds.
    release(&t.path().join("release"), "0.9.0", &[("kalka", KALKA_8)]);
    let before = snapshot(t.path());
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("kulman: plaz: not in the release"),
        "{}",
        stderr(&out)
    );
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn update_repairs_a_half_finished_update() {
    // An update killed after its first rename: kalka is 0.8.0, plaz and kulman are not, and
    // state.json still says 0.7.0.
    let (t, base, home) = installed();
    publish_0_8(t.path());
    fs::write(home.join("bin/kalka"), KALKA_8).unwrap();
    fs::write(home.join("bin/kulman"), "old").unwrap();
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out).len(), 3, "{}", stdout(&out));
    assert_eq!(
        state(&home),
        json!({"version": "0.8.0", "modules": ["kalka", "plaz"]})
    );
    assert_binary(&home, "kalka", KALKA_8);
    assert_binary(&home, "plaz", PLAZ_8);
    assert_binary(&home, "kulman", KULMAN_BYTES);
}

#[test]
fn update_when_current_says_up_to_date() {
    let (t, base, home) = installed();
    let before = snapshot(t.path());
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["up to date"]);
    assert_eq!(snapshot(t.path()), before);

    // A listed binary missing at the same version is put back.
    fs::remove_file(home.join("bin/plaz")).unwrap();
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "plaz", PLAZ);
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn update_does_not_downgrade() {
    let t = tempfile::tempdir().unwrap();
    let newer = release(&t.path().join("newer"), "0.10.0", &[("kalka", KALKA)]);
    let older = release(&t.path().join("release"), "0.7.0", &[("kalka", KALKA)]);
    let home = t.path().join("home");
    kulman(&home, &newer).arg("install").assert().success();
    let before = snapshot(t.path());
    let out = kulman(&home, &older).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        ["installed 0.10.0 is newer than the latest release 0.7.0; nothing changed"]
    );
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn update_with_nothing_installed_creates_nothing() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("kalka", KALKA)]);
    let home = t.path().join("home");
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("kulman: nothing installed; run kulman install"));
    assert!(!home.exists());
}

#[test]
fn remove_modules_drops_only_those() {
    let (t, base, home) = installed();
    let out = kulman(&home, &base)
        .args(["remove", "--modules", "plaz", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["removed plaz"]);
    assert!(!home.join("bin/plaz").exists());
    assert_binary(&home, "kalka", KALKA);
    assert_binary(&home, "kulman", KULMAN_BYTES);
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["kalka"]})
    );
    no_tmp_left(&home);

    let before = snapshot(t.path());
    let out = kulman(&home, &base)
        .args(["remove", "--modules", "plaz"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("kulman: plaz is not installed"));
    assert_eq!(snapshot(t.path()), before);

    // A listed binary already missing counts as removed.
    fs::remove_file(home.join("bin/kalka")).unwrap();
    kulman(&home, &base)
        .args(["remove", "--modules", "kalka"])
        .assert()
        .success();
    assert_eq!(state(&home), json!({"version": "0.7.0", "modules": []}));
    assert_binary(&home, "kulman", KULMAN_BYTES);
}

#[test]
fn remove_everything_leaves_foreign_files() {
    let (t, base, home) = installed();
    fs::write(home.join("notes.txt"), "mine").unwrap();
    fs::write(home.join("bin/other"), "mine too").unwrap();
    let out = kulman(&home, &base).arg("remove").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        ["removed kalka", "removed plaz", "removed kulman"]
    );
    let left: Vec<_> = snapshot(&home).into_keys().collect();
    assert_eq!(
        left,
        [
            Path::new("bin"),
            Path::new("bin/other"),
            Path::new("notes.txt")
        ]
    );
    assert!(t.path().join("release").exists());
}

#[test]
fn remove_everything_removes_an_empty_home() {
    let (t, base, home) = installed();
    // A stale temp entry is kulman's own; a listed binary already missing counts as removed.
    fs::create_dir(home.join(".tmp-stale")).unwrap();
    fs::remove_file(home.join("bin/plaz")).unwrap();
    let outside = snapshot(&t.path().join("user-home"));
    kulman(&home, &base)
        .args(["remove", "--yes"])
        .assert()
        .success();
    assert!(!home.exists());
    assert_eq!(snapshot(&t.path().join("user-home")), outside);

    let out = kulman(&home, &base).arg("remove").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["nothing installed"]);
    assert!(!home.exists());

    // A remove killed after state.json, before `.lock`: the next one finishes the job.
    kulman(&home, &base).arg("install").assert().success();
    for name in [
        "bin/kalka",
        "bin/plaz",
        "bin/kulman",
        "manifest.json",
        "state.json",
    ] {
        fs::remove_file(home.join(name)).unwrap();
    }
    let out = kulman(&home, &base).arg("remove").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["nothing installed"]);
    assert!(!home.exists());
}

#[test]
fn remove_kulman_by_name_is_usage() {
    let (t, base, home) = installed();
    let before = snapshot(t.path());
    let out = kulman(&home, &base)
        .args(["remove", "--modules", "kalka,kulman"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(snapshot(t.path()), before);

    let out = kulman(&home, &base)
        .args(["remove", "--modules", "../x"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("kulman: bad name in --modules: ../x"));
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn remove_refuses_a_path_like_name_in_the_state() {
    let (t, base, home) = installed();
    // bin/../../victim is this file beside home.
    fs::write(t.path().join("victim"), "keep").unwrap();
    fs::write(
        home.join("state.json"),
        br#"{"version":"0.7.0","modules":["kalka","../../victim"]}"#,
    )
    .unwrap();
    let before = snapshot(t.path());
    for args in [
        &["remove"][..],
        &["remove", "--modules", "kalka"],
        &["update"],
    ] {
        let out = kulman(&home, &base).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("kulman: bad name in state.json: ../../victim"),
            "{}",
            stderr(&out)
        );
        assert_eq!(snapshot(t.path()), before, "{args:?}");
    }
}

#[test]
fn two_installs_at_once_keep_both_modules() {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("kalka", KALKA), ("plaz", PLAZ)],
    );
    let home = t.path().join("home");
    let user = t.path().join("user-home");
    fs::create_dir(&user).unwrap();
    let start = |module: &str| {
        Command::new(assert_cmd::cargo::cargo_bin("kulman"))
            .env("KULMAN_HOME", &home)
            .env("KULMAN_RELEASE_URL", &base)
            .env("HOME", &user)
            .args(["install", "--modules", module])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let children = [start("kalka"), start("plaz")];
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
    }
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["kalka", "plaz"]})
    );
    assert_binary(&home, "kalka", KALKA);
    assert_binary(&home, "plaz", PLAZ);
    assert_binary(&home, "kulman", KULMAN_BYTES);
}

/// Moves `home/bin` to `outside` and links `home/bin` to it.
fn link_bin(t: &Path, home: &Path) -> std::path::PathBuf {
    let outside = t.join("outside");
    fs::rename(home.join("bin"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, home.join("bin")).unwrap();
    outside
}

#[test]
fn remove_refuses_a_symlinked_bin() {
    let (t, base, home) = installed();
    link_bin(t.path(), &home);
    let before = snapshot(t.path());
    let expect = format!(
        "kulman: {} is not a directory kulman can use",
        home.join("bin").display()
    );
    for args in [
        &["remove"][..],
        &["remove", "--modules", "kalka"],
        &["update"],
    ] {
        let out = kulman(&home, &base).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
        assert_eq!(snapshot(t.path()), before, "{args:?}");
    }
}

#[test]
fn remove_refuses_a_directory_in_place_of_a_binary() {
    let (t, base, home) = installed();
    fs::remove_file(home.join("bin/plaz")).unwrap();
    fs::create_dir(home.join("bin/plaz")).unwrap();
    fs::write(home.join("bin/plaz/inner"), "x").unwrap();
    let before = snapshot(t.path());
    let expect = format!("kulman: {} is not a file", home.join("bin/plaz").display());
    for args in [&["remove"][..], &["remove", "--modules", "kalka,plaz"]] {
        let out = kulman(&home, &base).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
        assert_eq!(snapshot(t.path()), before, "{args:?}");
    }
}

#[test]
fn a_state_naming_kulman_or_a_module_twice_is_malformed() {
    for (modules, expect) in [
        (
            json!(["kalka", "kulman"]),
            "kulman: bad name in state.json: kulman",
        ),
        (
            json!(["kalka", "plaz", "kalka"]),
            "kulman: state.json is malformed: kalka is listed twice",
        ),
    ] {
        let (t, base, home) = installed();
        let bad = json!({"version": "0.7.0", "modules": modules});
        fs::write(home.join("state.json"), bad.to_string()).unwrap();
        let before = snapshot(t.path());
        for args in [&[][..], &["update"], &["remove"]] {
            let out = kulman(&home, &base).args(args).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
            assert!(stderr(&out).contains(expect), "{}", stderr(&out));
            assert_eq!(snapshot(t.path()), before, "{args:?}");
        }
    }
}

#[test]
fn update_adds_what_an_installed_module_now_requires() {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("kalka", KALKA), ("plaz", PLAZ)],
    );
    let home = t.path().join("home");
    kulman(&home, &base)
        .args(["install", "--modules", "kalka"])
        .assert()
        .success();
    publish_0_8(t.path());
    common::edit_manifest(&t.path().join("release"), "0.8.0", |m| {
        m["modules"][0]["requires"] = json!(["plaz"]);
    });
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        state(&home),
        json!({"version": "0.8.0", "modules": ["kalka", "plaz"]})
    );
    assert_binary(&home, "kalka", KALKA_8);
    assert_binary(&home, "plaz", PLAZ_8);
}

#[test]
fn remove_on_a_home_with_no_state_deletes_no_file() {
    for args in [&["remove"][..], &["remove", "--modules", "kalka"]] {
        let (t, base, home) = installed();
        // Not kulman's by any record: no state.json.
        fs::remove_file(home.join("state.json")).unwrap();
        fs::remove_file(home.join("bin/kalka")).unwrap();
        fs::remove_file(home.join("bin/plaz")).unwrap();
        fs::write(home.join("notes.txt"), "mine").unwrap();
        let mut before = snapshot(&home);
        before.remove(Path::new(".lock"));
        let out = kulman(&home, &base).args(args).output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        let second = format!(
            "{} holds files kulman has no record of; left as they are",
            home.display()
        );
        assert_eq!(lines(&out), ["nothing installed", second.as_str()]);
        assert_eq!(snapshot(&home), before, "{args:?}");
        assert!(t.path().join("release").exists());
    }
}

#[test]
fn update_refuses_a_directory_in_place_of_a_binary() {
    let (t, base, home) = installed();
    publish_0_8(t.path());
    fs::remove_file(home.join("bin/plaz")).unwrap();
    fs::create_dir(home.join("bin/plaz")).unwrap();
    fs::write(home.join("bin/plaz/inner"), "x").unwrap();
    let before = snapshot(t.path());
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!("kulman: {} is not a file", home.join("bin/plaz").display());
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn update_when_current_does_not_need_every_module_in_the_release() {
    let (t, base, home) = installed();
    common::edit_manifest(&t.path().join("release"), "0.7.0", |m| {
        m["modules"]
            .as_array_mut()
            .unwrap()
            .retain(|x| x["name"] != "plaz");
    });
    let before = snapshot(t.path());
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["up to date"]);
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn remove_without_state_keeps_stale_temp_entries() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("kalka", KALKA)]);
    let home = t.path().join("home");
    fs::create_dir_all(home.join(".tmp-x")).unwrap();
    fs::write(home.join(".lock"), "").unwrap();
    fs::write(home.join(".tmp-x/f"), "not kulman's by any record").unwrap();
    let out = kulman(&home, &base).arg("remove").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(home.join(".tmp-x/f")).unwrap(),
        "not kulman's by any record"
    );
}
