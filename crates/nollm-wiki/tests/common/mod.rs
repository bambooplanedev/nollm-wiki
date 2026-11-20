//! A release laid out in a temp dir, and a `nollm-wiki` that never leaves it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-unknown-linux-musl"];

/// The bytes the helper packs as nollm-wiki's own binary.
pub const NOLLM_WIKI_BYTES: &[u8] = b"#!/bin/sh\necho nollm-wiki\n";

/// Lays out `latest/download/manifest.json` and `download/v<version>/` (manifest, one
/// `.tar.gz` per module and for nollm-wiki, `SHA256SUMS`) under `dir`; returns the `file://` base.
/// wiki and plaz are `default: true`, any other module `default: false`.
pub fn release(dir: &Path, version: &str, modules: &[(&str, &[u8])]) -> String {
    let vdir = version_dir(dir, version);
    fs::create_dir_all(&vdir).unwrap();
    let mut sums = String::new();
    let mut entry = |name: &str, bytes: &[u8]| {
        let stage = tempfile::tempdir().unwrap();
        fs::write(stage.path().join(name), bytes).unwrap();
        let file = archive_name(name, version);
        pack(stage.path(), &[name], &vdir.join(&file));
        let sum = sha256(&vdir.join(&file));
        sums.push_str(&format!("{sum}  {file}\n"));
        let archives: serde_json::Map<String, Value> = TARGETS
            .iter()
            .map(|t| (t.to_string(), json!({ "file": file, "sha256": sum })))
            .collect();
        Value::Object(archives)
    };
    let nollm_wiki = entry("nollm-wiki", NOLLM_WIKI_BYTES);
    let modules: Vec<Value> = modules
        .iter()
        .map(|(name, bytes)| {
            json!({
                "name": name,
                "summary": format!("the {name} module"),
                "default": *name == "wiki" || *name == "plaz",
                "requires": [],
                "options": [],
                "archives": entry(name, bytes),
            })
        })
        .collect();
    let manifest =
        json!({ "version": version, "modules": modules, "nollm-wiki": { "archives": nollm_wiki } });
    fs::write(vdir.join("SHA256SUMS"), sums).unwrap();
    write_manifest(dir, version, &manifest);
    format!("file://{}", dir.display())
}

pub fn version_dir(dir: &Path, version: &str) -> PathBuf {
    dir.join("download").join(format!("v{version}"))
}

pub fn archive_name(name: &str, version: &str) -> String {
    format!("{name}-v{version}.tar.gz")
}

pub fn archive_path(dir: &Path, version: &str, name: &str) -> PathBuf {
    version_dir(dir, version).join(archive_name(name, version))
}

/// Writes `manifest` to both places a release holds it.
pub fn write_manifest(dir: &Path, version: &str, manifest: &Value) {
    let bytes = serde_json::to_vec_pretty(manifest).unwrap();
    let latest = dir.join("latest").join("download");
    fs::create_dir_all(&latest).unwrap();
    fs::write(latest.join("manifest.json"), &bytes).unwrap();
    fs::write(version_dir(dir, version).join("manifest.json"), &bytes).unwrap();
}

pub fn read_manifest(dir: &Path, version: &str) -> Value {
    serde_json::from_slice(&fs::read(version_dir(dir, version).join("manifest.json")).unwrap())
        .unwrap()
}

/// Changes the manifest in both places.
pub fn edit_manifest(dir: &Path, version: &str, edit: impl FnOnce(&mut Value)) {
    let mut manifest = read_manifest(dir, version);
    edit(&mut manifest);
    write_manifest(dir, version, &manifest);
}

/// Sets every target's `sha256` of module `name` to `sum`.
pub fn set_sha256(dir: &Path, version: &str, name: &str, sum: &str) {
    edit_manifest(dir, version, |m| {
        let module = m["modules"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|x| x["name"] == name)
            .unwrap();
        for archive in module["archives"].as_object_mut().unwrap().values_mut() {
            archive["sha256"] = json!(sum);
        }
    });
}

/// Replaces module `name`'s archive by `bytes` and makes the manifest's checksum match them.
pub fn replace_archive(dir: &Path, version: &str, name: &str, bytes: &[u8]) {
    let path = archive_path(dir, version, name);
    fs::write(&path, bytes).unwrap();
    set_sha256(dir, version, name, &sha256(&path));
}

/// `tar -czf out -C dir members…` with the system tar (on macOS without `._` members).
pub fn pack(dir: &Path, members: &[&str], out: &Path) {
    let status = std::process::Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .arg("-czf")
        .arg(out)
        .arg("-C")
        .arg(dir)
        .args(members)
        .status()
        .unwrap();
    assert!(status.success(), "tar failed");
}

pub fn sha256(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).unwrap()))
}

/// `nollm-wiki` with `NOLLM_WIKI_HOME=home`, the release, `HOME` a temp dir beside `home`, stdin an empty closed pipe (not a terminal).
pub fn nollm_wiki(home: &Path, release: &str) -> Command {
    let user = home.with_file_name("user-home");
    fs::create_dir_all(&user).unwrap();
    let mut cmd = Command::cargo_bin("nollm-wiki").unwrap();
    cmd.env("NOLLM_WIKI_HOME", home)
        .env("NOLLM_WIKI_RELEASE_URL", release)
        .env("HOME", &user)
        .write_stdin(Vec::new());
    cmd
}

/// Starts `nollm-wiki <args>` (set up as [`nollm-wiki`] does, stdin null) while the caller holds
/// `<home>/.lock`; asserts that its first stderr line says it waits for that lock (within 10 s,
/// so a run that waits silently fails the test instead of hanging it) and that it is still
/// running. Returns it, and the rest of its stderr once it ends.
pub fn started_waiting(
    home: &Path,
    release: &str,
    args: &[&str],
) -> (std::process::Child, std::thread::JoinHandle<String>) {
    use std::io::{BufRead, Read};
    let user = home.with_file_name("user-home");
    fs::create_dir_all(&user).unwrap();
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("nollm-wiki"))
        .env("NOLLM_WIKI_HOME", home)
        .env("NOLLM_WIKI_RELEASE_URL", release)
        .env("HOME", &user)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut err = std::io::BufReader::new(child.stderr.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let rest = std::thread::spawn(move || {
        let mut line = String::new();
        let _ = err.read_line(&mut line);
        let _ = tx.send(line);
        let mut rest = String::new();
        let _ = err.read_to_string(&mut rest);
        rest
    });
    let line = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("no line on stderr while the lock is held");
    let expect = format!(
        "nollm-wiki: waiting for another nollm-wiki run ({})\n",
        home.join(".lock").display()
    );
    assert_eq!(line, expect);
    assert!(child.try_wait().unwrap().is_none(), "it did not wait");
    (child, rest)
}

pub fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn state(home: &Path) -> Value {
    serde_json::from_slice(&fs::read(home.join("state.json")).unwrap()).unwrap()
}

/// What every command says of a `state.json` it cannot read.
pub fn not_a_state(home: &Path) -> String {
    format!(
        "nollm-wiki: {} is not a state file nollm-wiki wrote; fix it, or delete it and run nollm-wiki install",
        home.join("state.json").display()
    )
}

pub fn assert_binary(home: &Path, name: &str, bytes: &[u8]) {
    let path = home.join("bin").join(name);
    let meta = fs::symlink_metadata(&path).unwrap();
    assert!(meta.file_type().is_file(), "{name} is not a regular file");
    assert_eq!(meta.permissions().mode() & 0o7777, 0o755, "{name} mode");
    assert_eq!(fs::read(&path).unwrap(), bytes, "{name} bytes");
}

pub fn no_tmp_left(home: &Path) {
    for entry in fs::read_dir(home).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(
            !name.to_string_lossy().starts_with(".tmp-"),
            "{} left",
            name.display()
        );
    }
}

/// Every path under `root` with its kind, mode and bytes (a symlink's bytes are its target).
pub fn snapshot(root: &Path) -> BTreeMap<PathBuf, (&'static str, u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    if root.exists() {
        walk(root, root, &mut out);
    }
    out
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (&'static str, u32, Vec<u8>)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let meta = fs::symlink_metadata(&path).unwrap();
        let mode = meta.permissions().mode();
        let rel = path.strip_prefix(root).unwrap().to_path_buf();
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap();
            out.insert(
                rel,
                (
                    "symlink",
                    mode,
                    target.into_os_string().into_encoded_bytes(),
                ),
            );
        } else if meta.is_dir() {
            out.insert(rel, ("dir", mode, Vec::new()));
            walk(root, &path, out);
        } else {
            out.insert(rel, ("file", mode, fs::read(&path).unwrap()));
        }
    }
}

/// Packs `bytes` as `nollm-wiki-v<version>-<target>.tar.gz` for every target, points the manifest's
/// `nollm-wiki` entry at them and lists them in `SHA256SUMS`, which it also copies to
/// `latest/download/`: the layout `install.sh` reads.
pub fn per_target_nollm_wiki(dir: &Path, version: &str, bytes: &[u8]) {
    let vdir = version_dir(dir, version);
    let stage = tempfile::tempdir().unwrap();
    fs::write(stage.path().join("nollm-wiki"), bytes).unwrap();
    let mut sums = fs::read_to_string(vdir.join("SHA256SUMS")).unwrap();
    let mut archives = serde_json::Map::new();
    for target in TARGETS {
        let file = format!("nollm-wiki-v{version}-{target}.tar.gz");
        pack(stage.path(), &["nollm-wiki"], &vdir.join(&file));
        let sum = sha256(&vdir.join(&file));
        sums.push_str(&format!("{sum}  {file}\n"));
        archives.insert(target.to_string(), json!({ "file": file, "sha256": sum }));
    }
    fs::write(vdir.join("SHA256SUMS"), &sums).unwrap();
    fs::write(dir.join("latest/download/SHA256SUMS"), &sums).unwrap();
    edit_manifest(dir, version, |m| {
        m["nollm-wiki"]["archives"] = Value::Object(archives);
    });
}

/// `PATH` with a new folder `links` first, holding a symlink to `<bin>/<name>` for each of `names`
/// (made before the binaries exist, so an install can follow), then the test's own.
pub fn path_linking(links: &Path, bin: &Path, names: &[&str]) -> std::ffi::OsString {
    fs::create_dir_all(links).unwrap();
    for name in names {
        std::os::unix::fs::symlink(bin.join(name), links.join(name)).unwrap();
    }
    let own = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::iter::once(links.to_path_buf()).chain(std::env::split_paths(&own));
    std::env::join_paths(dirs).unwrap()
}

/// `$HOME/.local/bin` of the user home [`nollm-wiki`] sets for `home` (not created).
pub fn local_bin(home: &Path) -> PathBuf {
    home.with_file_name("user-home").join(".local/bin")
}

/// `PATH` with `first` before the test's own entries.
pub fn path_first(first: impl Into<PathBuf>) -> std::ffi::OsString {
    let own = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(first.into()).chain(std::env::split_paths(&own))).unwrap()
}
