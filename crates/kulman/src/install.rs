//! `kulman install` and `kulman update`: the release choice, staging and commit.

use std::collections::BTreeSet;

use anyhow::{bail, Result};

use crate::{ask_choose, ask_yes, is_installed, on_path, release, store, Usage};
use release::Manifest;
use store::{Home, State};

/// Default release location; `KULMAN_RELEASE_URL` overrides it.
const RELEASES: &str = "https://github.com/bambooplanedev/kulman/releases";

/// The release base, checked: a usage error when it is neither https:// nor file://.
pub(super) fn release_base() -> Result<String> {
    let base = std::env::var("KULMAN_RELEASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| RELEASES.to_string());
    let base = base.trim_end_matches('/');
    release::check_base(base).map_err(|e| Usage(e.to_string()))?;
    Ok(base.to_string())
}

/// The build target of this machine.
fn target() -> Result<&'static str> {
    use std::env::consts::{ARCH, OS};
    Ok(match (OS, ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        _ => bail!("no build for {OS}/{ARCH}"),
    })
}

/// The usage error for a `--modules` name the release does not have.
pub(super) fn unknown_module(manifest: &Manifest, name: &str) -> anyhow::Error {
    let known: Vec<&str> = manifest.modules.iter().map(|m| m.name.as_str()).collect();
    let known = known.join(", ");
    Usage(format!(
        "--modules: unknown module {name:?} (known: {known})"
    ))
    .into()
}

/// `wanted`, or the manifest's defaults, plus everything they require.
fn choose(manifest: &Manifest, wanted: Option<Vec<String>>) -> Result<BTreeSet<String>> {
    let mut todo = match wanted {
        Some(names) => {
            if let Some(name) = names.iter().find(|n| manifest.module(n).is_none()) {
                return Err(unknown_module(manifest, name));
            }
            names
        }
        None => manifest
            .modules
            .iter()
            .filter(|m| m.default)
            .map(|m| m.name.clone())
            .collect(),
    };
    let mut chosen = BTreeSet::new();
    while let Some(name) = todo.pop() {
        let Some(module) = manifest.module(&name) else {
            bail!("a module requires {name}, which is not in the release");
        };
        if chosen.insert(name) {
            todo.extend(module.requires.iter().cloned());
        }
    }
    Ok(chosen)
}

/// Refuses an installed version that differs from the release's.
fn check_version(state: &State, manifest: &Manifest) -> Result<()> {
    let (have, want) = (&state.version, &manifest.version);
    let order = release::version_key(have).cmp(&release::version_key(want));
    if order.is_lt() {
        bail!("version {have} is installed; run kulman update");
    }
    if order.is_gt() {
        bail!(
            "version {have} is installed, newer than {want}; \
             run kulman remove, then kulman install --version {want}"
        );
    }
    Ok(())
}

pub(super) fn install(wanted: Option<Vec<String>>, version: Option<&str>, ask: bool) -> Result<()> {
    let base = release_base()?;
    let home_path = store::home_path()?;
    let target = target()?;
    let home = Home::open(home_path)?;
    match place(&home, &base, wanted, version, target, ask) {
        Ok(true) => {}
        // Declined: as if never run.
        done => {
            home.abandon();
            return done.map(|_| ());
        }
    }
    let bin = home.path.join("bin");
    if !on_path(&bin) {
        println!("add to PATH: {}", bin.display());
    }
    println!("next: run \"kulman setup --scope repo\" (or local) in a project");
    Ok(())
}

/// Under the lock: reads the installed state and picks the manifest, stages every chosen
/// module not installed and every listed binary missing from `bin/`, and only when all are
/// ready commits them. With `ask`, asks first; `false` when declined or nothing was needed.
fn place(
    home: &Home,
    base: &str,
    wanted: Option<Vec<String>>,
    version: Option<&str>,
    target: &str,
    ask: bool,
) -> Result<bool> {
    let state = State::load(&home.path)?;
    // Stale temp entries are cleaned only in a home a state records as kulman's.
    if state.is_some() {
        home.clean_tmp()?;
    }
    // Installed at V with no --version: V's manifest, so a newer release does not block adding
    // modules; that one, or a failed check for it, is only said.
    let mut note = None;
    let (manifest, raw) = match (version, &state) {
        (None, Some(state)) => match release::manifest(base, None, release::WAIT) {
            // Not newer: used as it is (an older one is refused below).
            Ok((m, raw))
                if release::version_key(&m.version) <= release::version_key(&state.version) =>
            {
                (m, raw)
            }
            latest => {
                note = Some(match latest {
                    Ok((m, _)) => format!("newer release {}: kulman update", m.version),
                    Err(e) => format!(
                        "could not check for a newer release ({base}: {})",
                        e.root_cause()
                    ),
                });
                release::manifest(base, Some(&state.version), release::WAIT)?
            }
        },
        _ => release::manifest(base, version, release::WAIT)?,
    };
    let wanted = match wanted {
        None if ask => {
            // A terminal run that will be refused on the version is refused before the question.
            if let Some(state) = &state {
                check_version(state, &manifest)?;
            }
            let items: Vec<_> = (manifest.modules.iter())
                .map(|m| (m.name.as_str(), m.summary.as_str(), m.default))
                .collect();
            Some(ask_choose("Modules to install:", &items, false)?)
        }
        wanted => wanted,
    };
    let chosen = choose(&manifest, wanted)?;
    let mut modules = BTreeSet::new();
    // At the same version, an installed kulman is the release's already.
    let kulman_too = state.is_none() || !home.path.join("bin/kulman").is_file();
    if let Some(state) = state {
        check_version(&state, &manifest)?;
        modules.extend(state.modules);
    }
    let bin = home.path.join("bin");
    let missing = |name: &String| !bin.join(name).is_file();
    for name in chosen
        .iter()
        .filter(|n| !modules.contains(*n) && !missing(n))
    {
        let path = bin.join(name);
        eprintln!(
            "kulman: replacing {}: kulman has no record of installing it",
            path.display()
        );
    }
    // Only a module the state lists counts as installed when its file is there: any other file
    // at bin/<name> is replaced by the fetched, checked release. kulman last, renamed last.
    let mut names: Vec<String> = chosen
        .union(&modules)
        .filter(|n| !modules.contains(*n) || missing(n))
        .cloned()
        .collect();
    if ask && !names.is_empty() {
        let question = format!("Install {} module(s) into {}?", names.len(), bin.display());
        if !ask_yes(&question, true)? {
            println!("nothing changed");
            return Ok(false);
        }
    }
    if kulman_too {
        names.push("kulman".to_string());
    }
    // Every chosen module is listed and in bin/ (so the state stays as it is): nothing to write.
    if names.is_empty() && home.path.join("manifest.json").is_file() {
        let chosen: Vec<_> = chosen.into_iter().collect();
        println!(
            "already installed: {} ({})",
            chosen.join(", "),
            manifest.version
        );
        if let Some(note) = note {
            println!("{note}");
        }
        return Ok(false);
    }
    let staged = store::stage(home, base, &manifest, target, &names)?;
    modules.extend(chosen);
    let state = State {
        version: manifest.version.clone(),
        modules: modules.into_iter().collect(),
    };
    store::commit(home, &staged, &raw, &state)?;
    for s in &staged {
        println!("installed {} {}", s.name, manifest.version);
    }
    if let Some(note) = note {
        println!("{note}");
    }
    Ok(true)
}

pub(super) fn update(ask: bool) -> Result<()> {
    let base = release_base()?;
    let home_path = store::home_path()?;
    let target = target()?;
    if !is_installed(&home_path)? {
        bail!(
            "nothing installed in {}; run kulman install",
            home_path.display()
        );
    }
    let (manifest, raw) = release::manifest(&base, None, release::WAIT)?;
    let home = Home::open(home_path)?;
    let result = renew(&home, &base, &manifest, &raw, target, ask);
    if result.is_err() {
        home.abandon();
    }
    result
}

/// Under the lock: unless already current or newer, stages kulman and every installed module
/// at the latest version, and only when all are ready commits them (kulman last). With `ask`,
/// a move to a newer release is asked first.
fn renew(
    home: &Home,
    base: &str,
    manifest: &Manifest,
    raw: &[u8],
    target: &str,
    ask: bool,
) -> Result<()> {
    let Some(state) = State::load(&home.path)? else {
        bail!(
            "nothing installed in {}; run kulman install",
            home.path.display()
        );
    };
    home.clean_tmp()?;
    let (have, latest) = (&state.version, &manifest.version);
    let bin = home.path.join("bin");
    let listed = state.modules.iter().map(String::as_str).chain(["kulman"]);
    match release::version_key(have).cmp(&release::version_key(latest)) {
        std::cmp::Ordering::Greater => {
            println!("installed {have} is newer than the latest release {latest}; nothing changed");
            return Ok(());
        }
        std::cmp::Ordering::Equal
            if listed.clone().all(|n| bin.join(n).is_file())
                && home.path.join("manifest.json").is_file() =>
        {
            println!("up to date");
            return Ok(());
        }
        _ => {}
    }
    // The installed modules, plus what they require in the latest release.
    for name in &state.modules {
        manifest.archive(name, target)?;
    }
    let modules = choose(manifest, Some(state.modules.clone()))?;
    if ask && release::version_key(latest) > release::version_key(have) {
        let question = format!("Update {} module(s) to {latest}?", modules.len());
        if !ask_yes(&question, true)? {
            println!("nothing changed");
            return Ok(());
        }
    }
    let mut names: Vec<String> = modules.iter().cloned().collect();
    names.push("kulman".to_string());
    let staged = store::stage(home, base, manifest, target, &names)?;
    let state = State {
        version: latest.clone(),
        modules: modules.into_iter().collect(),
    };
    store::commit(home, &staged, raw, &state)?;
    for s in &staged {
        println!("updated {} {latest}", s.name);
    }
    Ok(())
}
