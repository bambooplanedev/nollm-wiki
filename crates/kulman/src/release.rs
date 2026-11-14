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
    pub default: bool,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub archives: BTreeMap<String, Archive>,
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

    /// The archive of `name` (a module, or `kulman`) for `target`.
    pub fn archive(&self, name: &str, target: &str) -> Result<&Archive> {
        let archives = if name == "kulman" {
            &self.kulman.archives
        } else {
            &self
                .modules
                .iter()
                .find(|m| m.name == name)
                .ok_or_else(|| anyhow!("{name}: not in the release"))?
                .archives
        };
        archives
            .get(target)
            .ok_or_else(|| anyhow!("{name}: no build for {target}"))
    }
}

/// `X.Y.Z`, three runs of digits.
pub fn is_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The three integers of a version, for comparing.
pub fn version_key(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| p.parse().unwrap_or(u64::MAX))
        .collect()
}

/// A module name: `^[a-z][a-z0-9-]*$`.
pub fn check_name(name: &str, place: &str) -> Result<()> {
    let ok = name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
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

/// Checks a release base (trailing slashes already trimmed): `https://…`, or `file://` with no
/// host or `localhost`; no `?` or `#`, which would garble the paths appended to it.
pub fn check_base(base: &str) -> Result<()> {
    let https = base.strip_prefix("https://").is_some_and(|h| !h.is_empty());
    if !base.contains(['?', '#']) && (https || local_path(base).is_some()) {
        Ok(())
    } else {
        bail!("KULMAN_RELEASE_URL must start with https:// or file:// (got {base})")
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

/// The longest a manifest fetch may take: `kulman` with no command must not hang on it.
const MANIFEST_TIME: Duration = Duration::from_secs(20);

/// The HTTPS client: 15 s to connect, 60 s for any one read, `whole` for the whole request
/// (none for an archive), redirects followed but never to plain `http://`.
fn agent(whole: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(whole)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .into()
}

/// Copies `url` into `out`, from disk for `file://`, failing once more than `limit` bytes
/// arrive or, over https, once `whole` has passed.
fn download(url: &str, out: &mut impl Write, limit: u64, whole: Option<Duration>) -> Result<()> {
    let reader: Box<dyn Read> = match local_path(url) {
        Some(path) => Box::new(File::open(path)?),
        None => Box::new(agent(whole).get(url).call()?.into_body().into_reader()),
    };
    if io::copy(&mut reader.take(limit + 1), out)? > limit {
        bail!("larger than {} MiB", limit >> 20);
    }
    Ok(())
}

/// The manifest of `version`, or of the latest release; with its bytes as read.
pub fn manifest(base: &str, version: Option<&str>) -> Result<(Manifest, Vec<u8>)> {
    let url = match version {
        Some(v) => format!("{base}/download/v{v}/manifest.json"),
        None => format!("{base}/latest/download/manifest.json"),
    };
    let mut bytes = Vec::new();
    download(&url, &mut bytes, MANIFEST_LIMIT, Some(MANIFEST_TIME))
        .map_err(|e| anyhow!("manifest.json: download failed: {e}"))?;
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
        .and_then(|mut out| download(&url, &mut out, ARCHIVE_LIMIT, None));
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

/// The SHA-256 of a file, as lowercase hex.
pub fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
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
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    File::open(&path)?.sync_all()?;
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
        assert!(agent(None).config().https_only());
        assert_eq!(agent(None).config().timeouts().global, None);
        let whole = Some(MANIFEST_TIME);
        assert_eq!(agent(whole).config().timeouts().global, whole);
    }

    #[test]
    fn a_download_over_the_limit_fails() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, [0u8; 11]).unwrap();
        let url = format!("file://{}", file.display());
        assert!(download(&url, &mut Vec::new(), 11, None).is_ok());
        let err = download(&url, &mut Vec::new(), 10, None).unwrap_err();
        assert!(err.to_string().starts_with("larger than"), "{err}");
    }
}
