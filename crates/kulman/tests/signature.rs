//! The signature of `manifest.json` on `file://` trees. No test can hold a signature by the
//! release key (its secret half is the owner's): a valid signature is only ever refused here.
mod common;

use std::fs;
use std::path::Path;

use common::{kulman, release, snapshot, stderr, stdout, version_dir};

const KALKA: &[u8] = b"#!/bin/sh\necho kalka\n";
const UNCHECKED: &str = "signature not checked: local release tree";
const REFUSED: &str = "kulman: manifest.json: signature does not verify\nnothing changed\n";

/// A well-formed signature by the TEST key, which is not the release key.
const TEST_SIGNED: &[u8] = include_bytes!("data/signing/file.txt.minisig");

/// Signatures that must all be refused.
fn bad_signatures() -> Vec<Vec<u8>> {
    vec![
        TEST_SIGNED.to_vec(),
        b"garbage".to_vec(),
        Vec::new(),
        vec![0xff, 0xfe, b'\n'],
        // Past the 4 KiB limit.
        [TEST_SIGNED, &[b'\n'; 4096]].concat(),
    ]
}

/// Puts `sig` next to the manifest of `version` and the latest one.
fn sign(dir: &Path, version: &str, sig: &[u8]) {
    fs::write(dir.join("latest/download/manifest.json.minisig"), sig).unwrap();
    fs::write(version_dir(dir, version).join("manifest.json.minisig"), sig).unwrap();
}

fn tree() -> (tempfile::TempDir, String) {
    let t = tempfile::tempdir().unwrap();
    let base = release(&t.path().join("release"), "0.7.0", &[("kalka", KALKA)]);
    (t, base)
}

fn count(text: &str) -> usize {
    text.lines().filter(|l| *l == UNCHECKED).count()
}

#[test]
fn an_unsigned_local_tree_installs_and_says_so_once() {
    let (t, base) = tree();
    let home = t.path().join("home");
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).lines().next(), Some(UNCHECKED));
    assert_eq!(count(&stdout(&out)), 1, "{}", stdout(&out));
    assert_eq!(stderr(&out), "");
    // Installed: the latest is checked, then the installed version's manifest is read.
    release(&t.path().join("release"), "0.8.0", &[("kalka", KALKA)]);
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(count(&stdout(&out)), 1, "{}", stdout(&out));
}

#[test]
fn a_bad_signature_refuses_install_and_changes_nothing() {
    for sig in bad_signatures() {
        let (t, base) = tree();
        sign(&t.path().join("release"), "0.7.0", &sig);
        let home = t.path().join("home");
        for args in [&["install"][..], &["install", "--version", "0.7.0"]] {
            let mut cmd = kulman(&home, &base); // makes the user's home first
            let before = snapshot(t.path());
            let out = cmd.args(args).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{args:?} {}", stderr(&out));
            assert_eq!(stderr(&out), REFUSED, "{args:?}");
            assert_eq!(stdout(&out), "", "{args:?}");
            assert_eq!(snapshot(t.path()), before, "{args:?}");
        }
    }
}

#[test]
fn a_bad_signature_of_the_installed_version_refuses_install() {
    let (t, base) = tree();
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    let dir = t.path().join("release");
    // The latest (0.8.0) is unsigned and newer; the installed 0.7.0's manifest is badly signed.
    release(&dir, "0.8.0", &[("kalka", KALKA)]);
    fs::write(
        version_dir(&dir, "0.7.0").join("manifest.json.minisig"),
        TEST_SIGNED,
    )
    .unwrap();
    let before = snapshot(t.path());
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(stderr(&out), REFUSED);
    assert_eq!(stdout(&out), format!("{UNCHECKED}\n"));
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn a_bad_latest_signature_is_only_said_by_an_install_that_has_a_version() {
    let (t, base) = tree();
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    let dir = t.path().join("release");
    fs::write(dir.join("latest/download/manifest.json.minisig"), "garbage").unwrap();
    let out = kulman(&home, &base).arg("install").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!(
            "{UNCHECKED}\nalready installed: kalka (0.7.0)\n\
             could not check for a newer release ({base}: manifest.json: signature does not verify)\n"
        )
    );
}

#[test]
fn a_bad_signature_refuses_update_and_changes_nothing() {
    for sig in bad_signatures() {
        let (t, base) = tree();
        let home = t.path().join("home");
        kulman(&home, &base).arg("install").assert().success();
        let dir = t.path().join("release");
        release(&dir, "0.8.0", &[("kalka", KALKA)]);
        sign(&dir, "0.8.0", &sig);
        let before = snapshot(t.path());
        let out = kulman(&home, &base).arg("update").output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        assert_eq!(stderr(&out), REFUSED);
        assert_eq!(stdout(&out), "");
        assert_eq!(snapshot(t.path()), before);
    }
}

#[test]
fn an_unsigned_local_tree_updates_and_says_so_once() {
    let (t, base) = tree();
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    release(&t.path().join("release"), "0.8.0", &[("kalka", KALKA)]);
    let out = kulman(&home, &base).arg("update").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).lines().next(), Some(UNCHECKED));
    assert_eq!(count(&stdout(&out)), 1, "{}", stdout(&out));
}

#[test]
fn status_says_a_bad_signature_as_a_failed_check() {
    let (t, base) = tree();
    let home = t.path().join("home");
    kulman(&home, &base).arg("install").assert().success();
    let out = kulman(&home, &base).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[lines.len() - 2..],
        [UNCHECKED, "up to date"],
        "{text}"
    );
    for sig in bad_signatures() {
        sign(&t.path().join("release"), "0.7.0", &sig);
        let before = snapshot(t.path());
        let out = kulman(&home, &base).output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        let text = stdout(&out);
        assert_eq!(count(&text), 0, "{text}");
        assert_eq!(
            text.lines().last().unwrap(),
            format!(
                "could not check for a newer release ({base}: manifest.json: signature does not verify)"
            )
        );
        assert_eq!(snapshot(t.path()), before);
    }
}
