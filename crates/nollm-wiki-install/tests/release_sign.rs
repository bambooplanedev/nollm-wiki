//! `scripts/release-sign.sh` in a copy of the checkout with a tag, against a fake `gh` (a draft
//! laid out in a temp dir) and a fake `minisign`: nothing reaches GitHub, and no key is used.
mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::{sha256, stderr, stdout, TARGETS};
use serde_json::json;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const VERSION: &str = "0.7.0";
const BINARIES: [&str; 3] = ["wiki", "plaz", "nollm-wiki-install"];
/// What the script runs, linked from this `PATH`; `gh` and `minisign` are fakes.
const TOOLS: [&str; 22] = [
    "sh",
    "git",
    "jq",
    "sha256sum",
    "shasum",
    "mktemp",
    "rm",
    "cmp",
    "comm",
    "sed",
    "sort",
    "grep",
    "cat",
    "dirname",
    "mkdir",
    "awk",
    "tr",
    "ls",
    "cp",
    "printf",
    "pwd",
    "env",
];

/// The releases address of `install.rs`, which the script must take as this repository's.
fn releases() -> String {
    let source =
        fs::read_to_string(Path::new(ROOT).join("crates/nollm-wiki-install/src/install.rs"))
            .unwrap();
    let start = source.find("const RELEASES: &str = \"").unwrap() + 24;
    let len = source[start..].find('"').unwrap();
    source[start..start + len].to_string()
}

fn archive(binary: &str, target: &str) -> String {
    format!("{binary}-v{VERSION}-{target}.tar.gz")
}

/// A temp root: `repo/` (the script, `install.sh` and `release/nollm-wiki.pub`, committed and tagged
/// v0.7.0), `gh/assets/` (the draft's 9 files) and `gh/draft` (there: the release is a draft),
/// `bin/` (the `PATH`), `home/` (the `HOME`). The fakes log to `gh.log`, `upload.args` and
/// `minisign.log`; `verify.code` is the exit code of `minisign -V`.
struct Fixture {
    t: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let f = Fixture {
            t: tempfile::tempdir().unwrap(),
        };
        f.repo();
        f.draft();
        f.tools();
        fake(&f.path("bin/gh"), &GH.replace("@F@", &f.root()));
        fake(&f.path("bin/minisign"), &MINISIGN.replace("@F@", &f.root()));
        fs::create_dir(f.path("home")).unwrap();
        f
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.t.path().join(rel)
    }

    fn root(&self) -> String {
        self.t.path().display().to_string()
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .current_dir(self.path("repo"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
    }

    fn repo(&self) {
        let repo = self.path("repo");
        for rel in [
            "scripts/release-sign.sh",
            "install.sh",
            "release/nollm-wiki.pub",
            "crates/nollm-wiki-install/src/install.rs",
        ] {
            fs::create_dir_all(repo.join(rel).parent().unwrap()).unwrap();
            fs::copy(Path::new(ROOT).join(rel), repo.join(rel)).unwrap();
        }
        self.git(&["init", "-q"]);
        self.git(&["add", "."]);
        self.git(&["commit", "-q", "-m", "release"]);
        self.git(&["tag", "v0.7.0"]);
    }

    /// The 9 files the release workflow uploads, consistent with each other.
    fn draft(&self) {
        let assets = self.path("gh/assets");
        fs::create_dir_all(&assets).unwrap();
        fs::write(self.path("gh/draft"), "true").unwrap();
        fs::write(
            self.path("gh/url"),
            format!("{}/tag/untagged-1", releases()),
        )
        .unwrap();
        let mut sums = String::new();
        let mut entries = |binary: &str| {
            let mut archives = serde_json::Map::new();
            for target in TARGETS {
                let file = archive(binary, target);
                fs::write(assets.join(&file), format!("{binary} for {target}\n")).unwrap();
                let sum = sha256(&assets.join(&file));
                sums.push_str(&format!("{sum}  {file}\n"));
                archives.insert(target.into(), json!({"file": file, "sha256": sum}));
            }
            serde_json::Value::Object(archives)
        };
        let modules: Vec<_> = ["wiki", "plaz"]
            .iter()
            .map(|m| json!({"name": m, "archives": entries(m)}))
            .collect();
        let manifest = json!({
            "version": VERSION,
            "modules": modules,
            "nollm-wiki-install": {"archives": entries("nollm-wiki-install")},
        });
        fs::write(assets.join("SHA256SUMS"), sums).unwrap();
        fs::write(
            assets.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::copy(
            Path::new(ROOT).join("install.sh"),
            assets.join("install.sh"),
        )
        .unwrap();
    }

    /// `bin/` holding a link to each of `TOOLS` this machine has (no `gh`, no `minisign`).
    fn tools(&self) {
        let bin = self.path("bin");
        fs::create_dir(&bin).unwrap();
        for tool in TOOLS {
            let found = Command::new("sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .unwrap();
            let real = String::from_utf8(found.stdout).unwrap();
            // Builtins (`printf`, `pwd`) print no path; `sha256sum` or `shasum` may be missing.
            if found.status.success() && real.starts_with('/') {
                std::os::unix::fs::symlink(real.trim(), bin.join(tool)).unwrap();
            }
        }
    }

    fn asset(&self, name: &str) -> PathBuf {
        self.path("gh/assets").join(name)
    }

    /// The script run from the temp root, stdin not a terminal, with `NOLLM_WIKI_SIGN_NO_TTY_CHECK`.
    fn sign(&self, args: &[&str]) -> Output {
        self.run(args, true)
    }

    fn run(&self, args: &[&str], skip_tty_check: bool) -> Output {
        let mut cmd = Command::new(self.path("bin/sh"));
        cmd.arg(self.path("repo/scripts/release-sign.sh"))
            .args(args)
            .current_dir(self.t.path())
            .env_clear()
            .env("PATH", self.path("bin"))
            .env("HOME", self.path("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null());
        if skip_tty_check {
            cmd.env("NOLLM_WIKI_SIGN_NO_TTY_CHECK", "1");
        }
        cmd.output().unwrap()
    }

    fn log(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.path(name))
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default()
    }

    /// Each `minisign` call, its arguments split.
    fn minisign_calls(&self) -> Vec<Vec<String>> {
        let calls = self.log("minisign.log");
        calls
            .iter()
            .map(|l| l.split('|').map(String::from).collect())
            .collect()
    }

    /// Exit 1 with `expect` on stderr, and nothing uploaded or published.
    fn refused(&self, out: &Output, expect: &str) {
        assert_eq!(out.status.code(), Some(1), "{}", stderr(out));
        let err = stderr(out);
        assert!(err.contains(expect), "{err}");
        assert!(!self.path("upload.args").exists(), "uploaded on: {err}");
        assert!(self.log("gh.log").iter().all(|l| !l.contains("edit")));
    }

    /// As `refused`, and before anything was signed.
    fn refused_unsigned(&self, out: &Output, expect: &str) {
        self.refused(out, expect);
        assert_eq!(self.minisign_calls(), Vec::<Vec<String>>::new());
    }
}

/// Logs each call; `release view` describes `gh/assets` (a draft while `gh/draft` is there; no
/// release without the folder), `release download` copies it, `release upload` writes its
/// arguments to `upload.args` and adds the files.
const GH: &str = r#"#!/bin/sh
F='@F@'
printf '%s\n' "$*" >>"$F/gh.log"
case "$1 $2" in
    'release view')
        [ -d "$F/gh/assets" ] || { echo 'release not found' >&2; exit 1; }
        if [ -e "$F/gh/draft" ]; then d=$(cat "$F/gh/draft"); else d=false; fi
        ls "$F/gh/assets" | jq -R . | jq -s --argjson d "$d" --arg u "$(cat "$F/gh/url")" \
            '{url: $u, isDraft: $d, assets: map({name: .})}'
        ;;
    'release download')
        mkdir -p "$5" && cp "$F/gh/assets/"* "$5/"
        ;;
    'release upload')
        printf '%s\n' "$@" >"$F/upload.args"
        shift 3
        for a in "$@"; do
            case $a in --*) ;; *) cp "$a" "$F/gh/assets/" ;; esac
        done
        ;;
    *) exit 1 ;;
esac
"#;

/// Logs each call, its arguments joined by `|`; `-S` writes `<-m file>.minisig`, `-V` exits with
/// the code in `verify.code` (0 when there is none).
const MINISIGN: &str = r#"#!/bin/sh
F='@F@'
(IFS='|'; printf '%s\n' "$*") >>"$F/minisign.log"
case $1 in
    -S)
        while [ "$#" -gt 0 ]; do [ "$1" = -m ] && m=$2; shift; done
        printf 'fake signature\n' >"$m.minisig"
        ;;
    -V) exit "$(cat "$F/verify.code" 2>/dev/null || echo 0)" ;;
esac
"#;

fn fake(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(ToString::to_string).collect()
}

/// The arguments of the signing calls and the verifying calls, the temp paths checked apart:
/// `-m` names the downloaded file, and `-x` its signature beside it.
fn assert_signed(f: &Fixture, key: Option<&str>) {
    let calls = f.minisign_calls();
    assert_eq!(calls.len(), 4, "{calls:?}");
    let dl = Path::new(&calls[0][calls[0].len() - 3]).parent().unwrap();
    let at = |name: &str| dl.join(name).display().to_string();
    for (call, name) in calls[..2].iter().zip(["manifest.json", "SHA256SUMS"]) {
        let mut want = strings(&["-S"]);
        if let Some(k) = key {
            want.extend(strings(&["-s", k]));
        }
        want.extend([
            "-m".into(),
            at(name),
            "-t".into(),
            format!("nollm-wiki {VERSION} {name}"),
        ]);
        assert_eq!(call, &want);
    }
    for (call, name) in calls[2..].iter().zip(["manifest.json", "SHA256SUMS"]) {
        let mut want = strings(&["-V", "-H", "-q", "-p", "release/nollm-wiki.pub", "-m"]);
        want.extend([at(name), "-x".into(), at(&format!("{name}.minisig"))]);
        assert_eq!(call, &want);
    }
    // The temp dir is gone.
    assert!(!dl.exists());
}

fn assert_uploaded(f: &Fixture, clobber: bool) {
    let args = f.log("upload.args");
    assert_eq!(args[..3], strings(&["release", "upload", "v0.7.0"]));
    assert!(args[3].ends_with("/manifest.json.minisig"), "{args:?}");
    assert!(args[4].ends_with("/SHA256SUMS.minisig"), "{args:?}");
    let rest: Vec<&str> = args[5..].iter().map(String::as_str).collect();
    assert_eq!(rest, if clobber { vec!["--clobber"] } else { vec![] });
    for sig in ["manifest.json.minisig", "SHA256SUMS.minisig"] {
        assert_eq!(
            fs::read_to_string(f.asset(sig)).unwrap(),
            "fake signature\n"
        );
    }
    assert!(f
        .log("gh.log")
        .iter()
        .all(|l| !l.starts_with("release edit")));
}

#[test]
fn a_draft_is_checked_signed_verified_and_uploaded_but_not_published() {
    let f = Fixture::new();
    let out = f.sign(&[VERSION]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_signed(&f, None);
    assert_uploaded(&f, false);
    let text = stdout(&out);
    for binary in BINARIES {
        for target in TARGETS {
            let file = archive(binary, target);
            let line = format!("  {}  {file}\n", sha256(&f.asset(&file)));
            assert!(text.contains(&line), "{text}");
        }
    }
    let repository = releases()
        .replace("https://github.com/", "")
        .replace("/releases", "");
    let release = format!("{}/tag/untagged-1", releases());
    assert!(
        text.starts_with(&format!("repository: {repository}\nrelease: {release}\n")),
        "{text}"
    );
    assert!(text.contains("with the key 87CC6E990C51AF83 (release/nollm-wiki.pub)\n"));
    assert!(text.contains("minisign asks for the password twice, once per file\n"));
    assert!(text.ends_with(
        "v0.7.0 is signed and still a draft (11 files). To publish it:\n    gh release edit \
         v0.7.0 --draft=false\nPublished, it is the latest release: the one install.sh installs.\n"
    ));
    // The release is read twice (before and after) and downloaded once; nothing else.
    let gh: Vec<String> = f
        .log("gh.log")
        .into_iter()
        .map(|l| l.split(' ').take(2).collect::<Vec<_>>().join(" "))
        .collect();
    assert_eq!(
        gh,
        strings(&[
            "release view",
            "release download",
            "release upload",
            "release view"
        ])
    );
}

#[test]
fn with_replace_the_signatures_are_made_again_and_clobbered() {
    let f = Fixture::new();
    for sig in ["manifest.json.minisig", "SHA256SUMS.minisig"] {
        fs::write(f.asset(sig), "old signature\n").unwrap();
    }
    let out = f.sign(&[VERSION, "--replace", "--key", "owner.key"]);
    assert!(out.status.success(), "{}", stderr(&out));
    // A relative key path is taken from where the script was started, passed on as it is.
    let cwd = f.t.path().canonicalize().unwrap();
    let key = cwd.join("owner.key").display().to_string();
    assert_signed(&f, Some(&key));
    assert_uploaded(&f, true);
}

#[test]
fn signatures_already_there_without_replace_are_refused() {
    let f = Fixture::new();
    for sig in ["manifest.json.minisig", "SHA256SUMS.minisig"] {
        fs::write(f.asset(sig), "old signature\n").unwrap();
    }
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        "release-sign.sh: v0.7.0 is already signed (SHA256SUMS.minisig manifest.json.minisig); \
         --replace signs it again\n",
    );
}

#[test]
fn a_draft_of_another_repository_is_refused() {
    let f = Fixture::new();
    let other = "https://github.com/someone/fork/releases/tag/untagged-1";
    fs::write(f.path("gh/url"), other).unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        &format!(
            "release-sign.sh: this draft belongs to {other}, not to {}\n",
            releases()
        ),
    );
    assert!(f
        .log("gh.log")
        .iter()
        .all(|l| !l.starts_with("release download")));
}

#[test]
fn an_address_that_only_begins_like_ours_is_refused() {
    let f = Fixture::new();
    let other = format!("{}X/tag/v{VERSION}", releases());
    fs::write(f.path("gh/url"), &other).unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        &format!(
            "release-sign.sh: this draft belongs to {other}, not to {}\n",
            releases()
        ),
    );
    assert!(f
        .log("gh.log")
        .iter()
        .all(|l| !l.starts_with("release download")));
    assert!(f
        .log("gh.log")
        .iter()
        .all(|l| !l.starts_with("release upload")));
}

#[test]
fn a_draft_state_that_is_not_true_or_false_is_refused() {
    let f = Fixture::new();
    fs::write(f.path("gh/draft"), "null").unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        "release-sign.sh: could not read the release's draft state\n",
    );
}

#[test]
fn a_published_release_is_refused() {
    let f = Fixture::new();
    fs::remove_file(f.path("gh/draft")).unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        "release-sign.sh: v0.7.0 is already published; only a draft is signed\n",
    );
}

#[test]
fn a_release_that_does_not_exist_is_refused() {
    let f = Fixture::new();
    fs::remove_dir_all(f.path("gh")).unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        "release-sign.sh: cannot read the release v0.7.0 (before signing); is the tag pushed and \
         the draft made?\n",
    );
}

#[test]
fn a_missing_file_is_refused() {
    let f = Fixture::new();
    fs::remove_file(f.asset("install.sh")).unwrap();
    fs::remove_file(f.asset(&archive("plaz", TARGETS[1]))).unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        "release-sign.sh: v0.7.0 lacks: install.sh plaz-v0.7.0-x86_64-unknown-linux-musl.tar.gz\n",
    );
}

#[test]
fn an_unexpected_file_is_refused_even_with_replace() {
    let f = Fixture::new();
    fs::write(f.asset("notes.txt"), "").unwrap();
    fs::write(f.asset("SHA256SUMS.minisig"), "").unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION, "--replace"]),
        "release-sign.sh: v0.7.0 holds unexpected files: notes.txt\n",
    );
}

#[test]
fn an_archive_whose_hash_is_not_in_sha256sums_is_refused() {
    let f = Fixture::new();
    let file = archive("wiki", TARGETS[0]);
    fs::write(f.asset(&file), "altered\n").unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        &format!("release-sign.sh: {file}: its SHA-256 is not the one in SHA256SUMS\n"),
    );
}

#[test]
fn a_manifest_hash_that_differs_is_refused() {
    let f = Fixture::new();
    let path = f.asset("manifest.json");
    let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    m["nollm-wiki-install"]["archives"][TARGETS[1]]["sha256"] = json!("0".repeat(64));
    fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        &format!(
            "release-sign.sh: {}: its SHA-256 is not the one in manifest.json\n",
            archive("nollm-wiki-install", TARGETS[1])
        ),
    );
}

#[test]
fn a_manifest_of_another_version_is_refused() {
    let f = Fixture::new();
    let path = f.asset("manifest.json");
    let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    m["version"] = json!("0.6.9");
    fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        "release-sign.sh: manifest.json states version 0.6.9, not 0.7.0\n",
    );
}

#[test]
fn sha256sums_with_a_line_too_many_is_refused() {
    let f = Fixture::new();
    let path = f.asset("SHA256SUMS");
    let mut sums = fs::read_to_string(&path).unwrap();
    sums.push_str(&format!("{}  extra.tar.gz\n", "0".repeat(64)));
    fs::write(&path, sums).unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        "release-sign.sh: SHA256SUMS does not list exactly the 6 archives of 0.7.0\n",
    );
}

#[test]
fn an_install_sh_that_is_not_the_tags_is_refused() {
    let f = Fixture::new();
    let mut bytes = fs::read(f.asset("install.sh")).unwrap();
    bytes.extend_from_slice(b"# changed\n");
    fs::write(f.asset("install.sh"), bytes).unwrap();
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        "release-sign.sh: install.sh of the draft is not the one at v0.7.0\n",
    );
}

#[test]
fn a_tag_not_in_the_checkout_is_refused() {
    let f = Fixture::new();
    f.git(&["tag", "-d", "v0.7.0"]);
    f.refused_unsigned(
        &f.sign(&[VERSION]),
        "release-sign.sh: the tag v0.7.0 is not in this checkout; run git fetch --tags\n",
    );
}

#[test]
fn a_signature_that_does_not_verify_is_not_uploaded() {
    let f = Fixture::new();
    fs::write(f.path("verify.code"), "1").unwrap();
    f.refused(
        &f.sign(&[VERSION]),
        "release-sign.sh: manifest.json: the new signature does not verify with \
         release/nollm-wiki.pub (signed with another key?); nothing uploaded\n",
    );
    // Both files were signed, then the first check stopped it.
    let calls = f.minisign_calls();
    let ops: Vec<&str> = calls.iter().map(|c| c[0].as_str()).collect();
    assert_eq!(ops, ["-S", "-S", "-V"]);
}

#[test]
fn a_missing_tool_is_named() {
    for tool in ["gh", "minisign", "jq"] {
        let f = Fixture::new();
        fs::remove_file(f.path("bin").join(tool)).unwrap();
        f.refused_unsigned(
            &f.sign(&[VERSION]),
            &format!("release-sign.sh: {tool} is required\n"),
        );
        assert_eq!(f.log("gh.log"), Vec::<String>::new());
    }
}

#[test]
fn without_a_terminal_it_is_refused_up_front() {
    let f = Fixture::new();
    f.refused_unsigned(
        &f.run(&[VERSION], false),
        "release-sign.sh: run it in a terminal: minisign asks there for the secret key's password\n",
    );
    assert_eq!(f.log("gh.log"), Vec::<String>::new());
}

#[test]
fn a_bad_version_or_argument_is_refused() {
    let f = Fixture::new();
    let usage =
        "release-sign.sh: usage: release-sign.sh <version> [--key <secret key file>] [--replace]\n";
    for (args, want) in [
        (&["0.7"][..], "release-sign.sh: bad version: 0.7\n"),
        (&["v0.7.0"], "release-sign.sh: bad version: v0.7.0\n"),
        (&["0.07.0"], "release-sign.sh: bad version: 0.07.0\n"),
        (&[VERSION, "--key"], usage),
        (&[VERSION, "-x"], usage),
    ] {
        let out = f.sign(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(stderr(&out), want, "{args:?}");
    }
    assert_eq!(f.log("gh.log"), Vec::<String>::new());
}
