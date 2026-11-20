//! `nollm-wiki-install install` and `nollm-wiki-install update`: the release choice, staging and commit.

use std::collections::BTreeSet;

use anyhow::{bail, Result};

use crate::{ask_choose, ask_yes, is_installed, path_hint, reachable, release, store, Usage};
use release::Manifest;
use store::{Home, State};

/// Default release location; `NOLLM_WIKI_RELEASE_URL` overrides it.
const RELEASES: &str = "https://github.com/bambooplanedev/nollm-wiki/releases";

/// The release base, checked: a usage error when it is neither https:// nor file://.
pub(super) fn release_base() -> Result<String> {
    let base = std::env::var("NOLLM_WIKI_RELEASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| RELEASES.to_string());
    let base = release::check_base(&base).map_err(|e| Usage(e.to_string()))?;
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
        bail!("version {have} is installed; run nollm-wiki-install update");
    }
    if order.is_gt() {
        bail!(
            "version {have} is installed, newer than {want}; \
             run nollm-wiki-install remove, then nollm-wiki-install install --version {want}"
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
    let modules = State::load(&home.path)?.map_or_else(Vec::new, |s| s.modules);
    let names = modules
        .iter()
        .map(String::as_str)
        .chain(["nollm-wiki-install"]);
    let nollm_wiki_install = if reachable(&bin, names) {
        "nollm-wiki-install".to_string()
    } else {
        println!("{}", path_hint(&bin));
        bin.join("nollm-wiki-install").display().to_string()
    };
    println!("next: run \"{nollm_wiki_install} setup --scope repo\" (or local) in a project");
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
    // Stale temp entries are cleaned only in a home a state records as nollm-wiki-install's.
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
                    Ok((m, _)) => format!("newer release {}: nollm-wiki-install update", m.version),
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
    // At the same version, an installed nollm-wiki-install is the release's already.
    let nollm_wiki_install_too =
        state.is_none() || !home.path.join("bin/nollm-wiki-install").is_file();
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
            "nollm-wiki-install: replacing {}: nollm-wiki-install has no record of installing it",
            path.display()
        );
    }
    // Only a module the state lists counts as installed when its file is there: any other file
    // at bin/<name> is replaced by the fetched, checked release. nollm-wiki-install last, renamed last.
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
    if nollm_wiki_install_too {
        names.push("nollm-wiki-install".to_string());
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
            "nothing installed in {}; run nollm-wiki-install install",
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

/// The question before an update to `latest`, naming each module it removes.
fn update_question(count: usize, latest: &str, gone: &[String]) -> String {
    let removes = if gone.is_empty() {
        String::new()
    } else {
        format!("; removes {}", gone.join(", "))
    };
    format!("Update {count} module(s) to {latest}{removes}?")
}

/// The installed `manifest.json` reads and lists every module of `state`: one it lacks is a
/// removal a committed update has not finished.
fn installed_has(home: &Home, state: &State) -> bool {
    let manifest = std::fs::read(home.path.join("manifest.json")).ok();
    let manifest = manifest.and_then(|b| Manifest::parse(&b).ok());
    manifest.is_some_and(|m| state.modules.iter().all(|n| m.module(n).is_some()))
}

/// Under the lock: unless already current or newer, stages nollm-wiki-install and every installed module
/// at the latest version, and only when all are ready commits them (nollm-wiki-install last). With `ask`,
/// a move to a newer release, or any removal, is asked first.
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
            "nothing installed in {}; run nollm-wiki-install install",
            home.path.display()
        );
    };
    home.clean_tmp()?;
    let (have, latest) = (&state.version, &manifest.version);
    let bin = home.path.join("bin");
    let listed = state
        .modules
        .iter()
        .map(String::as_str)
        .chain(["nollm-wiki-install"]);
    match release::version_key(have).cmp(&release::version_key(latest)) {
        std::cmp::Ordering::Greater => {
            println!("installed {have} is newer than the latest release {latest}; nothing changed");
            return Ok(());
        }
        std::cmp::Ordering::Equal
            if listed.clone().all(|n| bin.join(n).is_file()) && installed_has(home, &state) =>
        {
            println!("up to date");
            return Ok(());
        }
        _ => {}
    }
    // The installed modules, plus what they require in the latest release. One the release no
    // longer holds goes, the module that `replaces` it (if any) taking its place.
    let renamed = |old: &String| {
        (manifest.modules.iter())
            .find(|m| m.replaces.contains(old))
            .map(|m| m.name.clone())
    };
    let (kept, gone): (Vec<String>, Vec<String>) =
        (state.modules.iter().cloned()).partition(|name| manifest.module(name).is_some());
    let wanted: Vec<String> = kept
        .into_iter()
        .chain(gone.iter().filter_map(renamed))
        .collect();
    for name in &wanted {
        manifest.archive(name, target)?;
    }
    let modules = choose(manifest, Some(wanted))?;
    // A move to a newer release, or any removal, is asked first.
    let asked =
        ask && (release::version_key(latest) > release::version_key(have) || !gone.is_empty());
    if asked && !ask_yes(&update_question(modules.len(), latest, &gone), true)? {
        println!("nothing changed");
        return Ok(());
    }
    let mut names: Vec<String> = modules.iter().cloned().collect();
    names.push("nollm-wiki-install".to_string());
    let staged = store::stage(home, base, manifest, target, &names)?;
    let old_modules = state.modules.clone();
    // Committed still listing the modules that go, so a run cut off before their removal
    // finishes it; the removal then writes the state without them.
    let mut listed = modules.clone();
    listed.extend(gone.iter().cloned());
    let state = |modules: BTreeSet<String>| State {
        version: latest.clone(),
        modules: modules.into_iter().collect(),
    };
    store::commit(home, &staged, raw, &state(listed))?;
    for s in &staged {
        // A module the release newly requires was not there before; at the same version, files
        // are only put back.
        if s.name != "nollm-wiki-install" && !old_modules.contains(&s.name) {
            println!("installed {} {latest}", s.name);
        } else if have == latest {
            println!("repaired {} {latest}", s.name);
        } else {
            println!("updated {} {have} -> {latest}", s.name);
        }
    }
    // Only once what replaces them is in place.
    if !gone.is_empty() {
        store::remove_modules(home, &gone, &state(modules))?;
    }
    for old in &gone {
        match renamed(old) {
            Some(new) => println!(
                "removed {old}: renamed {new}; run \"nollm-wiki-install setup --scope repo\" \
                 (or local) again in each project set up with {old}"
            ),
            None => println!("removed {old}: not in release {latest}"),
        }
    }
    if have != latest {
        println!("restart running agent sessions; they keep the old binaries until then");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::update_question;

    #[test]
    fn the_update_question_names_what_it_removes() {
        assert_eq!(
            update_question(2, "0.8.0", &[]),
            "Update 2 module(s) to 0.8.0?"
        );
        let gone = ["oldmod".to_string(), "old".to_string()];
        assert_eq!(
            update_question(1, "0.8.0", &gone),
            "Update 1 module(s) to 0.8.0; removes oldmod, old?"
        );
    }
}
