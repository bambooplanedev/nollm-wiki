//! A release: its manifest, fetching an archive, checking it and unpacking its one binary.

use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// `manifest.json`: the release's only source of truth. Unknown fields are ignored.
#[derive(Deserialize)]
pub struct Manifest {
    pub version: String,
    pub modules: Vec<Module>,
    /// The installer itself: not a module, always installed.
    pub kulman: Installer,
}

#[derive(Deserialize)]
pub struct Module {
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub options: Vec<ModuleOption>,
    #[serde(default)]
    pub archives: BTreeMap<String, Archive>,
}

/// A choice a module's own `setup` offers, such as calque's `memory`; its summary is not read.
#[derive(Deserialize)]
pub struct ModuleOption {
    pub name: String,
    #[serde(default)]
    pub default: bool,
}

#[derive(Deserialize)]
pub struct Installer {
    pub archives: BTreeMap<String, Archive>,
}

#[derive(Deserialize)]
pub struct Archive {
    pub file: String,
    pub sha256: String,
}

impl Manifest {
    /// Parses and checks every name and file the manifest holds.
    pub fn parse(bytes: &[u8]) -> Result<Manifest> {
        let m: Manifest =
            serde_json::from_slice(bytes).context("manifest.json is not a valid manifest")?;
        if !is_version(&m.version) {
            bail!("manifest.json: bad version {:?}", m.version);
        }
        for module in &m.modules {
            check_name(&module.name, "manifest")?;
            // kulman is the installer, never a module.
            if module.name == "kulman" {
                bail!("bad name in manifest: kulman");
            }
            for name in &module.requires {
                check_name(name, "manifest")?;
            }
        }
        let archives = m.modules.iter().flat_map(|x| x.archives.values());
        for archive in archives.chain(m.kulman.archives.values()) {
            check_file(&archive.file, "manifest")?;
            let hex = archive.sha256.bytes().all(|b| b.is_ascii_hexdigit());
            if archive.sha256.len() != 64 || !hex {
                bail!("bad sha256 in manifest: {}", archive.sha256);
            }
        }
        Ok(m)
    }

    /// The module named `name`.
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.name == name)
    }

    /// The archive of `name` (a module, or `kulman`) for `target`.
    pub fn archive(&self, name: &str, target: &str) -> Result<&Archive> {
        let archives = if name == "kulman" {
            &self.kulman.archives
        } else {
            &self
                .module(name)
                .ok_or_else(|| anyhow!("{name}: not in the release"))?
                .archives
        };
        archives
            .get(target)
            .ok_or_else(|| anyhow!("{name}: no build for {target}"))
    }
}

/// `X.Y.Z`, three runs of digits, each with no leading zero (but `0`) and within `u64`.
pub fn is_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            p.bytes().all(|b| b.is_ascii_digit())
                && (*p == "0" || !p.starts_with('0'))
                && p.parse::<u64>().is_ok()
        })
}

/// The three integers of a version, for comparing.
pub fn version_key(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| p.parse().unwrap_or(u64::MAX))
        .collect()
}

/// A module name: `^[a-z][a-z0-9-]*$`.
pub fn is_name(name: &str) -> bool {
    name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `is_name`, or an error naming `place`.
pub fn check_name(name: &str, place: &str) -> Result<()> {
    if is_name(name) {
        Ok(())
    } else {
        bail!("bad name in {place}: {name}")
    }
}

/// An archive file name: `^[A-Za-z0-9._-]+$`, not `.` or `..`.
pub fn check_file(name: &str, place: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if ok {
        Ok(())
    } else {
        bail!("bad name in {place}: {name}")
    }
}

/// Checks a release base as given and returns it without trailing slashes: `https://…`, or
/// `file://` with no host or `localhost`; no `?` or `#`, which would garble the paths appended
/// to it. A refusal shows the base as given.
pub fn check_base(given: &str) -> Result<&str> {
    let base = given.trim_end_matches('/');
    let https = base.strip_prefix("https://").is_some_and(|h| !h.is_empty());
    if !base.contains(['?', '#']) && (https || local_path(base).is_some()) {
        Ok(base)
    } else {
        bail!("KULMAN_RELEASE_URL must start with https:// or file:// (got {given})")
    }
}

fn local_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    rest.starts_with('/').then(|| PathBuf::from(rest))
}

/// The most bytes read for `manifest.json`, and for one archive.
const MANIFEST_LIMIT: u64 = 1 << 20;
const ARCHIVE_LIMIT: u64 = 256 << 20;

/// How long a manifest fetch may take to connect, and in all.
#[derive(Clone, Copy)]
pub struct Wait {
    pub connect: Duration,
    pub whole: Duration,
}

/// For `install` and `update` (an archive: the same to connect, no limit in all).
pub const WAIT: Wait = Wait {
    connect: Duration::from_secs(15),
    whole: Duration::from_secs(20),
};

/// For `kulman` with no command, which must not hang on a release host that does not answer.
pub const STATUS_WAIT: Wait = Wait {
    connect: Duration::from_secs(5),
    whole: Duration::from_secs(8),
};

/// The HTTPS client: `connect` to connect, 60 s for any one read, `whole` for the whole request
/// (none for an archive), redirects followed but never to plain `http://`.
fn agent(connect: Duration, whole: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(whole)
        .timeout_connect(Some(connect))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .into()
}

/// Copies `url` into `out`, from disk for `file://`, failing once more than `limit` bytes
/// arrive or, over https, past the time limits.
fn download(
    url: &str,
    out: &mut impl Write,
    limit: u64,
    connect: Duration,
    whole: Option<Duration>,
) -> Result<()> {
    let reader: Box<dyn Read> = match local_path(url) {
        Some(path) => Box::new(File::open(path)?),
        None => Box::new(
            agent(connect, whole)
                .get(url)
                .call()?
                .into_body()
                .into_reader(),
        ),
    };
    if io::copy(&mut reader.take(limit + 1), out)? > limit {
        bail!("larger than {} MiB", limit >> 20);
    }
    Ok(())
}

/// The release key (a minisign public key file), compiled in: there is no other.
const PUBLIC_KEY: &str = include_str!("../../../release/kulman.pub");

/// The most bytes read for `manifest.json.minisig` (one is about 300).
const SIGNATURE_LIMIT: u64 = 4096;

/// Checks that `signature` (a minisign signature file) is `public_key`'s (a minisign public key
/// file: an `untrusted comment:` line and the key line) over `file`, trusted comment included.
/// Only the prehashed form minisign makes by default is taken, not the legacy one.
pub fn verify(public_key: &str, file: &[u8], signature: &str) -> Result<()> {
    let key = match public_key.lines().collect::<Vec<_>>()[..] {
        [comment, key] if comment.starts_with("untrusted comment: ") => {
            minisign_verify::PublicKey::from_base64(key)
        }
        _ => Err(minisign_verify::Error::InvalidEncoding),
    }
    .map_err(|e| anyhow!("bad public key: {e}"))?;
    let signature =
        minisign_verify::Signature::decode(signature).map_err(|e| anyhow!("bad signature: {e}"))?;
    key.verify(file, &signature, false)
        .map_err(|e| anyhow!("{e}"))
}

/// Why a signature was not fetched: absent (a 404), or any other failure, said with its cause
/// only (the callers' lines already name the release base).
fn unfetched(e: &anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<ureq::Error>() {
        Some(ureq::Error::StatusCode(404)) => anyhow!("manifest.json: signature is missing"),
        _ => anyhow!("manifest.json: signature could not be fetched: {e:#}"),
    }
}

/// Said once per run, however many manifests come from an unsigned local tree.
static UNCHECKED: std::sync::Once = std::sync::Once::new();

/// Checks `bytes`, fetched from `url`, against `<url>.minisig` and the release key. Over https
/// the signature is required (a 404 is "missing", any other failure "could not be fetched"); in a
/// `file://` tree an absent one is allowed and said once.
fn check_signature(url: &str, bytes: &[u8], wait: Wait) -> Result<()> {
    let url = format!("{url}.minisig");
    if let Some(Ok(false)) = local_path(&url).map(|p| p.try_exists()) {
        UNCHECKED.call_once(|| eprintln!("signature not checked: local release tree"));
        return Ok(());
    }
    let bad = || anyhow!("manifest.json: signature does not verify");
    let mut sig = Vec::new();
    if let Err(e) = download(
        &url,
        &mut sig,
        SIGNATURE_LIMIT,
        wait.connect,
        Some(wait.whole),
    ) {
        // Past the limit (download copies one byte more before it fails): there, but no signature.
        if sig.len() as u64 > SIGNATURE_LIMIT {
            return Err(bad());
        }
        return Err(unfetched(&e));
    }
    let sig = String::from_utf8(sig).map_err(|_| bad())?;
    verify(PUBLIC_KEY, bytes, &sig).map_err(|_| bad())
}

/// The manifest of `version`, or of the latest release; with its bytes as read. Nothing in them
/// is read before their signature is checked.
pub fn manifest(base: &str, version: Option<&str>, wait: Wait) -> Result<(Manifest, Vec<u8>)> {
    let url = match version {
        Some(v) => format!("{base}/download/v{v}/manifest.json"),
        None => format!("{base}/latest/download/manifest.json"),
    };
    let mut bytes = Vec::new();
    download(
        &url,
        &mut bytes,
        MANIFEST_LIMIT,
        wait.connect,
        Some(wait.whole),
    )
    .with_context(|| format!("manifest.json: download failed: {url}"))?;
    check_signature(&url, &bytes, wait)?;
    let m = Manifest::parse(&bytes)?;
    if let Some(v) = version {
        if m.version != v {
            bail!("manifest.json of {v} states version {}", m.version);
        }
    }
    Ok((m, bytes))
}

/// Streams `<base>/download/v<version>/<file>` into `dest`; on failure `dest` is removed.
pub fn fetch_to(base: &str, version: &str, file: &str, dest: &Path) -> Result<()> {
    let url = format!("{base}/download/v{version}/{file}");
    let result = File::create(dest)
        .map_err(anyhow::Error::from)
        .and_then(|mut out| download(&url, &mut out, ARCHIVE_LIMIT, WAIT.connect, None));
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result.context(url)
}

/// The SHA-256 of a file, as lowercase hex.
pub fn sha256(path: &Path) -> Result<String> {
    let named = || path.display().to_string();
    let mut file = File::open(path).with_context(named)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = file.read(&mut buf).with_context(named)?;
        if n == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buf[..n]);
    }
}

/// Unpacks member `name` of `archive` into the empty dir `into` with the system tar, and
/// returns it only if it is one regular file with a single link, now mode 0o755 and synced.
pub fn unpack_one(archive: &Path, name: &str, into: &Path) -> Result<PathBuf> {
    let out = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .arg("--no-same-owner")
        .arg(name)
        .output()
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => anyhow!("tar is required"),
            _ => anyhow!("{name}: cannot run tar: {e}"),
        })?;
    // 127: the exec failed in the child, where the spawner could not report it (seen under
    // x86_64 emulation on Linux). Neither GNU tar nor bsdtar exits 127 itself.
    if out.status.code() == Some(127) {
        bail!("tar is required");
    }
    if !out.status.success() {
        // GNU tar prints the cause last; bsdtar prints one line.
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty());
        bail!("{name}: cannot unpack: {}", last.unwrap_or("").trim());
    }
    let path = into.join(name);
    if !fs::symlink_metadata(&path).is_ok_and(|m| is_lone_file(&m)) {
        bail!("{name}: archive holds no regular file {name}");
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
        .and_then(|()| File::open(&path)?.sync_all())
        .with_context(|| path.display().to_string())?;
    Ok(path)
}

/// A regular file with no second name: not a symlink, a directory or a hard link.
fn is_lone_file(meta: &Metadata) -> bool {
    meta.file_type().is_file() && meta.nlink() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_three_plain_numbers_that_fit_u64() {
        let max = u64::MAX;
        for good in ["0.0.0", "0.7.0", "1.10.200", &format!("{max}.0.{max}")] {
            assert!(is_version(good), "{good}");
        }
        let past = format!("0.{max}0.0"); // ten times u64::MAX
        for bad in [
            "", "1", "1.2", "1.2.3.4", "1..3", "01.2.3", "1.02.3", "1.2.03", "00.1.2", "+1.2.3",
            "1.2.3 ", "1.2.x", &past,
        ] {
            assert!(!is_version(bad), "{bad}");
        }
    }

    #[test]
    fn only_a_lone_regular_file_passes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "x").unwrap();
        let meta = |p: &Path| fs::symlink_metadata(p).unwrap();
        assert!(is_lone_file(&meta(&file)));
        std::os::unix::fs::symlink(&file, dir.path().join("link")).unwrap();
        assert!(!is_lone_file(&meta(&dir.path().join("link"))));
        assert!(!is_lone_file(&meta(dir.path())));
        fs::hard_link(&file, dir.path().join("hard")).unwrap();
        assert!(!is_lone_file(&meta(&file)));
    }

    #[test]
    fn the_client_refuses_plain_http() {
        // Nothing listens on the discard port: were plain http allowed, the error would be a
        // refused connection, not the refusal of the scheme.
        let err = download(
            "http://127.0.0.1:9/x",
            &mut Vec::new(),
            10,
            WAIT.connect,
            None,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "configured for https only: http://127.0.0.1:9/x"
        );
    }

    #[test]
    fn a_manifest_without_summary_or_options_parses() {
        let m = Manifest::parse(
            br#"{"version": "0.1.0", "modules": [{"name": "calque"}],
            "kulman": {"archives": {}}, "later": 1}"#,
        )
        .unwrap();
        assert_eq!(m.modules[0].summary, "");
        assert!(m.modules[0].options.is_empty());
    }

    /// The test key pair's public half and what it signed (the secret half was discarded).
    const TEST_KEY: &str = include_str!("../tests/data/signing/test.pub");
    const FILE: &[u8] = include_bytes!("../tests/data/signing/file.txt");
    const SIGNATURE: &str = include_str!("../tests/data/signing/file.txt.minisig");

    /// The key id in a minisign key line (bytes 2..10 of the base64, little-endian), as minisign
    /// prints it.
    fn key_id(key_line: &str) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = 0u32;
        let mut n = 0;
        let mut bytes = Vec::new();
        for c in key_line.bytes().take(16) {
            bits = bits << 6 | ALPHABET.iter().position(|&a| a == c).unwrap() as u32;
            n += 6;
            if n >= 8 {
                n -= 8;
                bytes.push((bits >> n) as u8);
            }
        }
        format!(
            "{:016X}",
            u64::from_le_bytes(bytes[2..10].try_into().unwrap())
        )
    }

    #[test]
    fn the_compiled_in_key_parses() {
        // Its id is the one its comment line names.
        let (comment, key_line) = PUBLIC_KEY.split_once('\n').unwrap();
        let named = comment
            .strip_prefix("untrusted comment: minisign public key ")
            .unwrap();
        assert_eq!(key_id(key_line.trim_end()), named);
        assert_eq!(key_id(TEST_KEY.lines().nth(1).unwrap()), "C2C112C21D5877E9");
        // Parsed and compared by id: the test key's signature is refused as another key's.
        let err = verify(PUBLIC_KEY, FILE, SIGNATURE).unwrap_err();
        assert_eq!(
            err.to_string(),
            "The signature was created with a different key than the one provided"
        );
    }

    #[test]
    fn a_signature_by_the_key_verifies() {
        verify(TEST_KEY, FILE, SIGNATURE).unwrap();
        // CRLF line ends: lines() strips them, so the same signature still verifies.
        let crlf = SIGNATURE.replace('\n', "\r\n");
        verify(&TEST_KEY.replace('\n', "\r\n"), FILE, &crlf).unwrap();
    }

    #[test]
    fn anything_else_does_not_verify() {
        let other = include_str!("../tests/data/signing/other.txt.minisig");
        let legacy = include_str!("../tests/data/signing/file.txt.legacy.minisig");
        let altered_file = [FILE, b"x"].concat();
        let sig_line = SIGNATURE.lines().nth(1).unwrap();
        let mut flipped = sig_line.as_bytes().to_vec();
        flipped[20] = if flipped[20] == b'A' { b'B' } else { b'A' };
        let altered_sig = SIGNATURE.replace(sig_line, std::str::from_utf8(&flipped).unwrap());
        let altered_comment = SIGNATURE.replace("file:file.txt", "file:other.txt");
        for (file, sig) in [
            (&altered_file[..], SIGNATURE),
            (FILE, altered_sig.as_str()),
            (FILE, altered_comment.as_str()),
            (FILE, other),
            (FILE, legacy),
            (FILE, "garbage"),
            (FILE, ""),
            (FILE, &SIGNATURE[..SIGNATURE.len() / 2]),
        ] {
            assert!(verify(TEST_KEY, file, sig).is_err(), "{sig}");
        }
        // By another key: the release key did not make the test signature.
        assert!(verify(PUBLIC_KEY, FILE, SIGNATURE).is_err());
    }

    #[test]
    fn a_manifest_is_not_parsed_before_its_signature_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let latest = dir.path().join("latest/download");
        fs::create_dir_all(&latest).unwrap();
        fs::write(latest.join("manifest.json"), "attacker's text").unwrap();
        fs::write(latest.join("manifest.json.minisig"), SIGNATURE).unwrap();
        let base = format!("file://{}", dir.path().display());
        let err = manifest(&base, None, WAIT).err().unwrap();
        assert_eq!(
            format!("{err:#}"),
            "manifest.json: signature does not verify"
        );
    }

    #[test]
    fn over_https_a_signature_that_cannot_be_reached_could_not_be_fetched() {
        // Nothing listens on the loopback discard port: refused at once, no network used.
        let url = "https://127.0.0.1:9/latest/download/manifest.json";
        let err = check_signature(url, b"{}", STATUS_WAIT).unwrap_err();
        let text = format!("{err:#}");
        let start = "manifest.json: signature could not be fetched: ";
        assert!(text.starts_with(start), "{text}");
        assert!(!text.contains("127.0.0.1"), "{text}");
    }

    #[test]
    fn a_host_that_never_answers_ends_the_check_within_the_given_wait() {
        // Accepts the connection and stays silent: only the wait can end the fetch.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("https://{}", listener.local_addr().unwrap());
        let wait = Wait {
            connect: Duration::from_millis(300),
            whole: Duration::from_millis(400),
        };
        let start = std::time::Instant::now();
        let err = manifest(&base, None, wait)
            .err()
            .expect("no answer, no manifest");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(err.root_cause().to_string(), "timeout: connect");
    }

    #[test]
    fn only_a_404_is_a_missing_signature() {
        let said = |e: ureq::Error| unfetched(&e.into()).to_string();
        assert_eq!(
            said(ureq::Error::StatusCode(404)),
            "manifest.json: signature is missing"
        );
        assert_eq!(
            said(ureq::Error::StatusCode(500)),
            "manifest.json: signature could not be fetched: http status: 500"
        );
    }

    #[test]
    fn a_signature_that_is_a_directory_could_not_be_fetched() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("manifest.json.minisig")).unwrap();
        let url = format!("file://{}/manifest.json", dir.path().display());
        let err = check_signature(&url, b"{}", WAIT).unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.starts_with("manifest.json: signature could not be fetched: "),
            "{text}"
        );
        assert!(!text.contains(&dir.path().display().to_string()), "{text}");
    }

    #[test]
    fn a_public_key_is_exactly_a_comment_line_and_a_key_line() {
        let key_line = TEST_KEY.lines().nth(1).unwrap();
        for bad in [
            key_line.to_string(),
            format!("{TEST_KEY}\n"),
            format!("comment\n{key_line}\n"),
            format!("{TEST_KEY}extra\n"),
            String::new(),
        ] {
            let err = verify(&bad, FILE, SIGNATURE).unwrap_err();
            assert!(
                err.to_string().starts_with("bad public key"),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn a_download_over_the_limit_fails() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, [0u8; 11]).unwrap();
        let url = format!("file://{}", file.display());
        assert!(download(&url, &mut Vec::new(), 11, WAIT.connect, None).is_ok());
        let err = download(&url, &mut Vec::new(), 10, WAIT.connect, None).unwrap_err();
        assert!(err.to_string().starts_with("larger than"), "{err}");
    }
}
