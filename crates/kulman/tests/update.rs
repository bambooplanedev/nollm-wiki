mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{
    assert_binary, kulman, local_bin, no_tmp_left, path_first, path_linking, release, set_sha256,
    snapshot, state, stderr, stdout, version_dir, KULMAN_BYTES,
};
use serde_json::json;

const CALQUE: &[u8] = b"#!/bin/sh\necho calque\n";
const PLAZ: &[u8] = b"#!/bin/sh\necho plaz\n";
const CALQUE_8: &[u8] = b"#!/bin/sh\necho calque 0.8\n";
const PLAZ_8: &[u8] = b"#!/bin/sh\necho plaz 0.8\n";

/// A temp root with `release/` at 0.7.0 (calque and plaz) and `home` holding both.
fn installed() -> (tempfile::TempDir, String, std::path::PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("calque", CALQUE), ("plaz", PLAZ)],
    );
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    (t, base, home)
}

/// Publishes 0.8.0 (calque and plaz, new bytes) as the latest in the same release dir.
fn publish_0_8(t: &Path) {
    release(
        &t.join("release"),
        "0.8.0",
        &[("calque", CALQUE_8), ("plaz", PLAZ_8)],
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
        &[("calque", CALQUE), ("plaz", PLAZ)],
    );
    let home = t.path().join("home");
    let out = kulman(&home, &base).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let nothing = format!(
        "nothing installed in {}; run kulman install",
        home.display()
    );
    assert_eq!(lines(&out), [nothing]);
    assert!(!home.exists());

    kulman(&home, &base).arg("install").assert().success();
    let before = snapshot(t.path());
    let out = kulman(&home, &base).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let first = format!("kulman 0.7.0 in {}", home.display());
    let bin = home.join("bin").display().to_string();
    let hint = format!("{bin} is not on PATH; to type kulman in a terminal, add it:");
    let export = format!("    export PATH=\"{bin}:$PATH\"");
    assert_eq!(stderr(&out), "signature not checked: local release tree\n");
    assert_eq!(
        lines(&out),
        [
            first.as_str(),
            "calque",
            "plaz",
            hint.as_str(),
            export.as_str(),
            "up to date"
        ]
    );
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn status_says_when_bin_is_not_on_path() {
    let (_t, base, home) = installed();
    let bin = home.join("bin");
    let line = format!(
        "{} is not on PATH; to type kulman in a terminal, add it:",
        bin.display()
    );
    let out = kulman(&home, &base).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out)[3], line);
    assert_eq!(
        lines(&out)[4],
        format!("    export PATH=\"{}:$PATH\"", bin.display())
    );
    let own = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&own)))
            .unwrap();
    let out = kulman(&home, &base).env("PATH", path).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("on PATH"), "{}", stdout(&out));
}

#[test]
fn status_takes_binaries_linked_into_path_as_on_path() {
    let (t, base, home) = installed();
    let (bin, links) = (home.join("bin"), t.path().join("links"));
    let say = |names: &[&str]| {
        let path = path_linking(&links, &bin, names);
        let out = kulman(&home, &base).env("PATH", path).output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        let _ = fs::remove_dir_all(&links);
        stdout(&out)
    };
    assert!(!say(&["kulman", "calque", "plaz"]).contains("on PATH"));
    // One module not linked, or kulman not linked: still said.
    assert!(say(&["kulman", "calque"]).contains("is not on PATH"));
    assert!(say(&["calque", "plaz"]).contains("is not on PATH"));
    // The first `plaz` on PATH is another program: still said.
    let path = path_linking(&links, &bin, &["kulman", "calque"]);
    fs::write(links.join("plaz"), "#!/bin/sh\n").unwrap();
    let out = kulman(&home, &base).env("PATH", path).output().unwrap();
    assert!(stdout(&out).contains("is not on PATH"), "{}", stdout(&out));
}

#[test]
fn status_marks_a_listed_binary_that_is_missing() {
    let (_t, base, home) = installed();
    fs::remove_file(home.join("bin/plaz")).unwrap();
    let out = kulman(&home, &base).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out)[1..3],
        ["calque", "plaz  (missing: run kulman update)"]
    );
}

#[test]
fn status_announces_a_newer_release() {
    let (t, base, home) = installed();
    // 0.10.0 is newer than 0.7.0 only when the three parts are compared as integers.
    release(&t.path().join("release"), "0.10.0", &[("calque", CALQUE)]);
    let out = kulman(&home, &base).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out).last().unwrap(),
        "newer release: 0.10.0 (kulman update)"
    );
}

/// Status waits 5 s to connect (`release::STATUS_WAIT`), install and update 15 s (`release::WAIT`).
/// A host that accepts and never answers must end the check by the status wait: well under 12 s,
/// the middle between 5 and 15 plus margin for a loaded machine.
#[test]
fn status_gives_up_on_a_silent_host_by_its_own_wait() {
    let (_t, _, home) = installed();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("https://{}", listener.local_addr().unwrap());
    let start = std::time::Instant::now();
    let out = kulman(&home, &base).output().unwrap();
    let took = start.elapsed();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = lines(&out);
    let last = text.last().unwrap();
    assert!(
        last.starts_with(&format!("could not check for a newer release ({base}: ")),
        "{text:?}"
    );
    assert!(last.ends_with(')'), "{text:?}");
    assert!(took < std::time::Duration::from_secs(12), "{took:?}");
}

#[test]
fn status_survives_an_unreachable_release() {
    let (t, _, home) = installed();
    let missing = format!("file://{}", t.path().join("nowhere").display());
    let before = snapshot(t.path());
    for (base, start) in [
        (
            missing.as_str(),
            format!("could not check for a newer release ({missing}: "),
        ),
        (
            "http://example.invalid/releases",
            "could not check for a newer release (KULMAN_RELEASE_URL must start with ".to_string(),
        ),
    ] {
        let out = kulman(&home, base).output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        let text = lines(&out);
        assert_eq!(text.len(), 6, "{text:?}");
        assert!(text[5].starts_with(&start), "{text:?}");
        assert!(text[5].ends_with(')'), "{text:?}");
        assert!(!text[5].contains("download failed"), "{text:?}");
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
    assert_eq!(stderr(&out), "signature not checked: local release tree\n");
    assert_eq!(
        lines(&out),
        [
            "updated calque 0.7.0 -> 0.8.0",
            "updated plaz 0.7.0 -> 0.8.0",
            "updated kulman 0.7.0 -> 0.8.0",
            "restart running agent sessions; they keep the old binaries until then"
        ]
    );
    assert_eq!(
        state(&home),
        json!({"version": "0.8.0", "modules": ["calque", "plaz"]})
    );
    assert_binary(&home, "calque", CALQUE_8);
    assert_binary(&home, "plaz", PLAZ_8);
    assert_binary(&home, "kulman", KULMAN_BYTES);
    assert_eq!(
        fs::read(home.join("manifest.json")).unwrap(),
        [
            fs::read(version_dir(&t.path().join("release"), "0.8.0").join("manifest.json"))
                .unwrap(),
            b"\n".to_vec()
        ]
        .concat()
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
    assert_eq!(stderr(&out).lines().last(), Some("nothing changed"));
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn update_removes_a_module_the_release_no_longer_holds_and_says_so() {
    let (_t, base, home, dir) = installed_linked(&[]);
    release(
        &home.with_file_name("release"),
        "0.9.0",
        &[("calque", CALQUE_8)],
    );
    let out = kulman(&home, &base)
        .env("PATH", path_first(&dir))
        .args(["update", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "updated calque 0.7.0 -> 0.9.0",
            "updated kulman 0.7.0 -> 0.9.0",
            "removed plaz: not in release 0.9.0",
            "restart running agent sessions; they keep the old binaries until then"
        ]
    );
    assert_eq!(
        state(&home),
        json!({"version": "0.9.0", "modules": ["calque"]})
    );
    assert!(!home.join("bin/plaz").exists());
    assert!(fs::symlink_metadata(dir.join("plaz")).is_err());
    no_tmp_left(&home);
}

#[test]
fn update_whose_removal_fails_after_the_commit_finishes_it_on_the_next_run() {
    let (_t, base, home) = installed();
    release(
        &home.with_file_name("release"),
        "0.9.0",
        &[("calque", CALQUE_8)],
    );
    fs::remove_file(home.join("bin/plaz")).unwrap();
    fs::create_dir(home.join("bin/plaz")).unwrap();
    let out = kulman(&home, &base)
        .args(["update", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    // Committed: what moved is said, and the state still lists what is not yet removed.
    assert_eq!(
        lines(&out),
        [
            "updated calque 0.7.0 -> 0.9.0",
            "updated kulman 0.7.0 -> 0.9.0"
        ]
    );
    assert_eq!(
        state(&home),
        json!({"version": "0.9.0", "modules": ["calque", "plaz"]})
    );
    fs::remove_dir(home.join("bin/plaz")).unwrap();
    fs::write(home.join("bin/plaz"), PLAZ).unwrap();
    let out = kulman(&home, &base)
        .args(["update", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "repaired calque 0.9.0",
            "repaired kulman 0.9.0",
            "removed plaz: not in release 0.9.0"
        ]
    );
    assert_eq!(
        state(&home),
        json!({"version": "0.9.0", "modules": ["calque"]})
    );
    assert!(!home.join("bin/plaz").exists());
}

#[test]
fn update_puts_the_new_name_of_a_renamed_module_in_its_place() {
    let t = tempfile::tempdir().unwrap();
    let rel = t.path().join("release");
    let base = release(&rel, "0.7.0", &[("kalka", CALQUE), ("plaz", PLAZ)]);
    let home = t.path().join("home");
    let dir = local_bin(&home);
    fs::create_dir_all(&dir).unwrap();
    kulman(&home, &base)
        .env("PATH", path_first(&dir))
        .args(["install", "--modules", "kalka,plaz"])
        .assert()
        .success();
    publish_0_8(t.path());
    common::edit_manifest(&rel, "0.8.0", |m| {
        m["modules"][0]["replaces"] = json!(["kalka"]);
    });
    let out = kulman(&home, &base)
        .env("PATH", path_first(&dir))
        .args(["update", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "installed calque 0.8.0",
            "updated plaz 0.7.0 -> 0.8.0",
            "updated kulman 0.7.0 -> 0.8.0",
            "removed kalka: renamed calque; run \"kulman setup --scope repo\" (or local) \
             again in each project set up with kalka",
            "restart running agent sessions; they keep the old binaries until then"
        ]
    );
    assert_eq!(
        state(&home),
        json!({"version": "0.8.0", "modules": ["calque", "plaz"]})
    );
    assert_binary(&home, "calque", CALQUE_8);
    assert!(!home.join("bin/kalka").exists());
    assert!(fs::symlink_metadata(dir.join("kalka")).is_err());
    assert_eq!(
        fs::read_link(dir.join("calque")).unwrap(),
        home.join("bin/calque")
    );
    no_tmp_left(&home);
}

#[test]
fn update_when_current_says_up_to_date() {
    let (t, base, home) = installed();
    let before = snapshot(t.path());
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "signature not checked: local release tree\n");
    assert_eq!(lines(&out), ["up to date"]);
    assert_eq!(snapshot(t.path()), before);

    // A listed binary missing at the same version is put back.
    fs::remove_file(home.join("bin/plaz")).unwrap();
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_binary(&home, "plaz", PLAZ);
    assert_eq!(stderr(&out), "signature not checked: local release tree\n");
    assert_eq!(
        lines(&out),
        [
            "repaired calque 0.7.0",
            "repaired plaz 0.7.0",
            "repaired kulman 0.7.0"
        ]
    );
    assert_eq!(snapshot(t.path()), before);

    // So is manifest.json.
    fs::remove_file(home.join("manifest.json")).unwrap();
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "repaired calque 0.7.0",
            "repaired plaz 0.7.0",
            "repaired kulman 0.7.0"
        ]
    );
    assert!(home.join("manifest.json").is_file());
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn update_does_not_downgrade() {
    let t = tempfile::tempdir().unwrap();
    let newer = release(&t.path().join("newer"), "0.10.0", &[("calque", CALQUE)]);
    let older = release(&t.path().join("release"), "0.7.0", &[("calque", CALQUE)]);
    let home = t.path().join("home");
    kulman(&home, &newer).arg("install").assert().success();
    let before = snapshot(t.path());
    let out = kulman(&home, &older).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "signature not checked: local release tree\n");
    assert_eq!(
        lines(&out),
        ["installed 0.10.0 is newer than the latest release 0.7.0; nothing changed"]
    );
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn update_with_nothing_installed_creates_nothing() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("calque", CALQUE)]);
    let home = t.path().join("home");
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let expect = format!(
        "kulman: nothing installed in {}; run kulman install",
        home.display()
    );
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
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
    assert_binary(&home, "calque", CALQUE);
    assert_binary(&home, "kulman", KULMAN_BYTES);
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["calque"]})
    );
    no_tmp_left(&home);

    let before = snapshot(t.path());
    let out = kulman(&home, &base)
        .args(["remove", "--modules", "plaz", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("kulman: plaz is not installed"));
    assert_eq!(snapshot(t.path()), before);

    // A listed binary already missing counts as removed.
    fs::remove_file(home.join("bin/calque")).unwrap();
    kulman(&home, &base)
        .args(["remove", "--modules", "calque", "--yes"])
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
    let out = kulman(&home, &base)
        .args(["remove", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [
            "removed calque",
            "removed plaz",
            "removed kulman",
            "projects set up with kulman still name \"calque\" in their agent configs: run \"kulman \
             setup --scope repo --remove\" (or local) in each first"
        ]
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
fn remove_without_calque_says_nothing_of_projects() {
    let (_t, base, home) = installed();
    kulman(&home, &base)
        .args(["remove", "--modules", "calque", "--yes"])
        .assert()
        .success();
    let out = kulman(&home, &base)
        .args(["remove", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["removed plaz", "removed kulman"]);
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

    let out = kulman(&home, &base)
        .args(["remove", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["nothing installed"]);
    assert!(!home.exists());

    // A remove killed after state.json, before `.lock`: the next one finishes the job.
    kulman(&home, &base).arg("install").assert().success();
    for name in [
        "bin/calque",
        "bin/plaz",
        "bin/kulman",
        "manifest.json",
        "state.json",
    ] {
        fs::remove_file(home.join(name)).unwrap();
    }
    let out = kulman(&home, &base)
        .args(["remove", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines(&out), ["nothing installed"]);
    assert!(!home.exists());
}

/// A home at 0.7.0 whose manifest says plaz requires calque.
fn installed_with_requires() -> (tempfile::TempDir, String, std::path::PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("release");
    let base = release(&dir, "0.7.0", &[("calque", CALQUE), ("plaz", PLAZ)]);
    common::edit_manifest(&dir, "0.7.0", |m| {
        m["modules"][1]["requires"] = json!(["calque"]);
    });
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    (t, base, home)
}

#[test]
fn remove_refuses_a_module_another_installed_one_requires() {
    let (t, base, home) = installed_with_requires();
    let before = snapshot(t.path());
    let out = kulman(&home, &base)
        .args(["remove", "--modules", "calque", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("kulman: cannot remove calque: plaz requires it\n"));
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn remove_a_module_together_with_the_one_that_requires_it() {
    let (_t, base, home) = installed_with_requires();
    let out = kulman(&home, &base)
        .args(["remove", "--modules", "calque,plaz", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(state(&home), json!({"version": "0.7.0", "modules": []}));
}

#[test]
fn remove_kulman_by_name_is_usage() {
    let (t, base, home) = installed();
    let before = snapshot(t.path());
    let out = kulman(&home, &base)
        .args(["remove", "--modules", "calque,kulman", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "kulman: kulman is not a module; kulman remove without --modules removes everything\n"
    );
    assert_eq!(snapshot(t.path()), before);

    let out = kulman(&home, &base)
        .args(["remove", "--modules", "../x", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("kulman: --modules: bad name \"../x\""));
    assert_eq!(snapshot(t.path()), before);

    // Spaces around a name are not part of it.
    kulman(&home, &base)
        .args(["remove", "--modules", " plaz", "--yes"])
        .assert()
        .success();
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["calque"]})
    );
}

#[test]
fn two_installs_at_once_keep_both_modules() {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("calque", CALQUE), ("plaz", PLAZ)],
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
    let children = [start("calque"), start("plaz")];
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
    }
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["calque", "plaz"]})
    );
    assert_binary(&home, "calque", CALQUE);
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
    for args in [&["remove", "--yes"][..], &["update"]] {
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
    for args in [
        &["remove", "--yes"][..],
        &["remove", "--modules", "calque,plaz", "--yes"],
    ] {
        let out = kulman(&home, &base).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
        assert_eq!(snapshot(t.path()), before, "{args:?}");
    }
}

#[test]
fn remove_refuses_a_manifest_that_is_not_a_file_before_deleting_any_binary() {
    let (t, base, home) = installed();
    let manifest = home.join("manifest.json");
    fs::remove_file(&manifest).unwrap();
    fs::create_dir(&manifest).unwrap();
    fs::write(manifest.join("inner"), "x").unwrap();
    let before = snapshot(t.path());
    let out = kulman(&home, &base)
        .args(["remove", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expect = format!("kulman: {} is not a file kulman wrote", manifest.display());
    assert!(stderr(&out).contains(&expect), "{}", stderr(&out));
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn a_malformed_state_is_named_with_the_way_out_by_every_command() {
    for bad in [
        r#"{"version":"0.7.0","modules":["calque","kulman"]}"#,
        r#"{"version":"0.7.0","modules":["calque","plaz","calque"]}"#,
        r#"{"version":"0.7.0","modules":["../../victim"]}"#,
        r#"{"version":"#,
    ] {
        let (t, base, home) = installed();
        let expect = &common::not_a_state(&home);
        fs::write(home.join("state.json"), bad).unwrap();
        let before = snapshot(t.path());
        for args in [
            &[][..],
            &["install"],
            &["update"],
            &["remove", "--yes"],
            &["setup", "--scope", "repo"],
        ] {
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
        &[("calque", CALQUE), ("plaz", PLAZ)],
    );
    let home = t.path().join("home");
    kulman(&home, &base)
        .args(["install", "--modules", "calque"])
        .assert()
        .success();
    publish_0_8(t.path());
    common::edit_manifest(&t.path().join("release"), "0.8.0", |m| {
        m["modules"][0]["requires"] = json!(["plaz"]);
    });
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    // The version changed, so the restart line stays; the new module is not "updated".
    assert_eq!(stderr(&out), "signature not checked: local release tree\n");
    assert_eq!(
        lines(&out),
        [
            "updated calque 0.7.0 -> 0.8.0",
            "installed plaz 0.8.0",
            "updated kulman 0.7.0 -> 0.8.0",
            "restart running agent sessions; they keep the old binaries until then"
        ]
    );
    assert_eq!(
        state(&home),
        json!({"version": "0.8.0", "modules": ["calque", "plaz"]})
    );
    assert_binary(&home, "calque", CALQUE_8);
    assert_binary(&home, "plaz", PLAZ_8);
}

#[test]
fn remove_on_a_home_with_no_state_deletes_no_file() {
    for args in [
        &["remove", "--yes"][..],
        &["remove", "--modules", "calque", "--yes"],
    ] {
        let (t, base, home) = installed();
        // Not kulman's by any record: no state.json.
        fs::remove_file(home.join("state.json")).unwrap();
        fs::remove_file(home.join("bin/calque")).unwrap();
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
        let third = format!(
            "to remove the installer itself, delete {}",
            home.join("bin/kulman").display()
        );
        assert_eq!(lines(&out), ["nothing installed", &second, &third]);
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
    assert_eq!(stderr(&out), "signature not checked: local release tree\n");
    assert_eq!(lines(&out), ["up to date"]);
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn remove_without_state_keeps_stale_temp_entries() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("calque", CALQUE)]);
    let home = t.path().join("home");
    fs::create_dir_all(home.join(".tmp-x")).unwrap();
    fs::write(home.join(".lock"), "").unwrap();
    fs::write(home.join(".tmp-x/f"), "not kulman's by any record").unwrap();
    let out = kulman(&home, &base)
        .args(["remove", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(home.join(".tmp-x/f")).unwrap(),
        "not kulman's by any record"
    );
}

#[test]
fn remove_without_a_terminal_needs_yes() {
    let (t, base, home) = installed();
    let before = snapshot(t.path());
    for args in [&["remove"][..], &["remove", "--modules", "plaz"]] {
        let out = kulman(&home, &base).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        assert_eq!(
            stderr(&out),
            "kulman: remove needs --yes when there is no terminal\n",
            "{args:?}"
        );
        assert_eq!(stdout(&out), "", "{args:?}");
        assert_eq!(snapshot(t.path()), before, "{args:?}");
    }
    // The same command with --yes goes through: that is all --yes changes without a terminal.
    let out = kulman(&home, &base)
        .args(["remove", "--modules", "plaz", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        state(&home),
        json!({"version": "0.7.0", "modules": ["calque"]})
    );
    assert!(!home.join("bin/plaz").exists());
}

#[test]
fn remove_without_a_terminal_on_a_missing_home_is_usage_and_creates_nothing() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("calque", CALQUE)]);
    let home = t.path().join("home");
    let mut cmd = kulman(&home, &base);
    // All but the user's home, where an emulator (Rosetta) may keep a cache of its own.
    let tree = || {
        let mut s = snapshot(t.path());
        s.retain(|p, _| !p.starts_with("user-home"));
        s
    };
    let before = tree();
    let out = cmd.arg("remove").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(!home.exists());
    assert_eq!(tree(), before);
}

#[test]
fn remove_says_when_it_waits_for_another_run() {
    let (_t, base, home) = installed();
    let lock = fs::File::options()
        .write(true)
        .open(home.join(".lock"))
        .unwrap();
    lock.lock().unwrap();
    let (child, rest) = common::started_waiting(&home, &base, &["remove", "--yes"]);
    drop(lock);
    let out = child.wait_with_output().unwrap();
    let rest = rest.join().unwrap();
    assert!(out.status.success(), "{rest}");
    assert_eq!(rest, "");
    assert!(!home.exists());
}

#[test]
fn an_unreadable_state_is_named_with_its_cause_by_every_command() {
    let (t, base, home) = installed();
    // Read fails (EISDIR), for root too: the file is not judged, and not to be deleted.
    let path = home.join("state.json");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let before = snapshot(t.path());
    for args in [
        &[][..],
        &["install"],
        &["update"],
        &["remove", "--yes"],
        &["setup", "--scope", "repo"],
    ] {
        let out = kulman(&home, &base).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        // The notice about an unsigned local tree may come first.
        let err = stderr(&out).replace("signature not checked: local release tree\n", "");
        let expect = format!("kulman: {}: ", path.display());
        assert!(err.lines().next().unwrap().starts_with(&expect), "{err}");
        assert!(!err.contains("delete it"), "{err}");
        assert_eq!(snapshot(t.path()), before, "{args:?}");
    }
}

#[test]
fn remove_after_install_sh_alone_says_how_to_remove_kulman() {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("calque", CALQUE)]);
    let home = t.path().join("home");
    fs::create_dir_all(home.join("bin")).unwrap();
    fs::write(home.join("bin/kulman"), KULMAN_BYTES).unwrap();
    let before = snapshot(&home);
    let out = kulman(&home, &base)
        .args(["remove", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let last = format!(
        "to remove the installer itself, delete {}",
        home.join("bin/kulman").display()
    );
    assert_eq!(lines(&out), ["nothing installed", last.as_str()]);
    assert_eq!(snapshot(&home), before);
}

#[test]
fn status_command_equals_the_plain_form() {
    let (_t, base, home) = installed();
    let plain = kulman(&home, &base).output().unwrap();
    let named = kulman(&home, &base).arg("status").output().unwrap();
    assert!(named.status.success(), "{}", stderr(&named));
    assert_eq!(stdout(&named), stdout(&plain));
    assert_eq!(stderr(&named), stderr(&plain));
}

#[test]
fn status_finds_bin_on_path_through_a_symlink() {
    let (t, base, home) = installed();
    let link = t.path().join("bin-link");
    std::os::unix::fs::symlink(home.join("bin"), &link).unwrap();
    let own = std::env::var_os("PATH").unwrap_or_default();
    // A missing and a relative entry come first: neither is an error.
    let entries = [t.path().join("missing"), "relative".into(), link]
        .into_iter()
        .chain(std::env::split_paths(&own));
    let out = kulman(&home, &base)
        .env("PATH", std::env::join_paths(entries).unwrap())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("on PATH"), "{}", stdout(&out));
}

/// A temp root with `release/` at 0.7.0 and `home` holding calque and plaz, linked from
/// `$HOME/.local/bin` (returned last) except where `foreign` names a file put there first.
fn installed_linked(foreign: &[&str]) -> (tempfile::TempDir, String, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let base = release(
        &t.path().join("release"),
        "0.7.0",
        &[("calque", CALQUE), ("plaz", PLAZ)],
    );
    let home = t.path().join("home");
    let dir = local_bin(&home);
    fs::create_dir_all(&dir).unwrap();
    for name in foreign {
        fs::write(dir.join(name), "mine").unwrap();
    }
    kulman(&home, &base)
        .env("PATH", path_first(&dir))
        .arg("install")
        .assert()
        .success();
    (t, base, home, dir)
}

#[test]
fn remove_modules_deletes_only_their_links() {
    let (_t, base, home, dir) = installed_linked(&[]);
    // A binary already gone leaves its link dangling: still removed.
    fs::remove_file(home.join("bin/plaz")).unwrap();
    let out = kulman(&home, &base)
        .args(["remove", "--modules", "plaz", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    assert!(fs::symlink_metadata(dir.join("plaz")).is_err());
    for name in ["calque", "kulman"] {
        assert_eq!(
            fs::read_link(dir.join(name)).unwrap(),
            home.join("bin").join(name)
        );
    }
}

#[test]
fn remove_everything_deletes_every_link_of_its_own() {
    let (_t, base, home, dir) = installed_linked(&["calque"]);
    let out = kulman(&home, &base)
        .args(["remove", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    let left: Vec<_> = snapshot(&dir).into_keys().collect();
    assert_eq!(left, [Path::new("calque")]);
    assert_eq!(fs::read(dir.join("calque")).unwrap(), b"mine");
}

#[test]
fn update_names_only_the_link_it_made() {
    let (_t, base, home, dir) = installed_linked(&[]);
    fs::remove_file(dir.join("plaz")).unwrap();
    // A listed binary missing: the update repairs it, so it commits.
    fs::remove_file(home.join("bin/calque")).unwrap();
    let out = kulman(&home, &base)
        .env("PATH", path_first(&dir))
        .args(["update", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let linked = format!("kulman: linked plaz into {}", dir.display());
    assert_eq!(stderr(&out).lines().last(), Some(linked.as_str()));
    for name in ["calque", "plaz", "kulman"] {
        assert_eq!(
            fs::read_link(dir.join(name)).unwrap(),
            home.join("bin").join(name)
        );
    }
}
