//! A release: its manifest, fetching an archive, checking it and unpacking its one binary.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
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
            for name in &module.requires {
                check_name(name, "manifest")?;
            }
        }
        let archives = m.modules.iter().flat_map(|x| x.archives.values());
        for archive in archives.chain(m.kulman.archives.values()) {
            check_file(&archive.file, "manifest")?;
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

/// Checks a release base: `https://…`, or `file://` with no host or `localhost`.
pub fn check_base(base: &str) -> Result<()> {
    if base.starts_with("https://") || local_path(base).is_some() {
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

/// Reads `url` as a stream: from disk for `file://`, else over HTTPS (15 s to connect,
/// 60 s for any one read, no limit on the whole, redirects followed).
fn open(url: &str) -> Result<Box<dyn Read>> {
    if let Some(path) = local_path(url) {
        return Ok(Box::new(File::open(path)?));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .into();
    Ok(Box::new(agent.get(url).call()?.into_body().into_reader()))
}

/// The manifest of `version`, or of the latest release; with its bytes as read.
pub fn manifest(base: &str, version: Option<&str>) -> Result<(Manifest, Vec<u8>)> {
    let url = match version {
        Some(v) => format!("{base}/download/v{v}/manifest.json"),
        None => format!("{base}/latest/download/manifest.json"),
    };
    let mut bytes = Vec::new();
    open(&url)
        .and_then(|mut r| Ok(r.read_to_end(&mut bytes)?))
        .map_err(|e| anyhow!("manifest.json: download failed: {e}"))?;
    let m = Manifest::parse(&bytes)?;
    if let Some(v) = version {
        if m.version != v {
            bail!("manifest.json of {v} states version {}", m.version);
        }
    }
    Ok((m, bytes))
}

/// Streams `<base>/download/v<version>/<file>` into `dest`.
pub fn fetch_to(base: &str, version: &str, file: &str, dest: &Path) -> Result<()> {
    let mut reader = open(&format!("{base}/download/v{version}/{file}"))?;
    io::copy(&mut reader, &mut File::create(dest)?)?;
    Ok(())
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
/// returns it only if it is one regular file with a single link, now mode 0o755.
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
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!(
            "{name}: cannot unpack: {}",
            err.lines().next().unwrap_or("")
        );
    }
    let path = into.join(name);
    let meta = fs::symlink_metadata(&path);
    if !meta.is_ok_and(|m| m.file_type().is_file() && m.nlink() == 1) {
        bail!("{name}: archive holds no regular file {name}");
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(path)
}
