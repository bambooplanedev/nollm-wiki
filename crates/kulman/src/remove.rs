//! `kulman remove`: deletes the named modules, or everything kulman installed.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Result};

use crate::release::Manifest;
use crate::{ask_yes, asks, is_installed, store, Usage};
use store::{Home, State};

pub(super) fn remove(names: Option<Vec<String>>, yes: bool) -> Result<()> {
    for name in names.iter().flatten() {
        if name == "kulman" {
            let why = "kulman is not a module; kulman remove without --modules removes everything";
            return Err(Usage(why.to_string()).into());
        }
    }
    // The question's default is no: without a terminal to ask at, that is nothing to do.
    let ask = asks(yes);
    if !yes && !ask {
        let why = "remove needs --yes when there is no terminal";
        return Err(Usage(why.to_string()).into());
    }
    let home_path = store::home_path()?;
    // Without state.json, a home with a `.lock` may be left by a remove killed before its last
    // step: only that lock is removed below. Anything else is not opened, so nothing is created.
    if !is_installed(&home_path)? && !home_path.join(".lock").try_exists()? {
        nothing_installed(&home_path, false);
        return Ok(());
    }
    let home = Home::open(home_path)?;
    let result = uninstall(&home, names, ask);
    // A run that created home (it vanished while this run waited) found nothing: undo it.
    if result.is_err() || home.created {
        home.abandon();
    }
    result
}

/// `kulman remove` with no state: says so, whether home `holds` other files, and how to remove
/// a `bin/kulman` (install.sh's).
fn nothing_installed(home: &Path, holds: bool) {
    println!("nothing installed");
    if holds {
        let home = home.display();
        println!("{home} holds files kulman has no record of; left as they are");
    }
    let kulman = home.join("bin/kulman");
    if kulman.is_file() {
        println!(
            "to remove the installer itself, delete {}",
            kulman.display()
        );
    }
}

/// Under the lock: every name is checked (by `State::load` and against the state) before
/// anything is asked (with `ask`) or the first file is deleted.
fn uninstall(home: &Home, mut names: Option<Vec<String>>, ask: bool) -> Result<()> {
    let Some(mut state) = State::load(&home.path)? else {
        // No record of owning anything: no binary and no manifest is deleted, nothing asked.
        nothing_installed(&home.path, store::remove_lock(home)?);
        return Ok(());
    };
    if let Some(names) = &mut names {
        names.sort();
        names.dedup();
        if let Some(name) = names.iter().find(|n| !state.modules.contains(n)) {
            bail!("{name} is not installed");
        }
        // What stays must not lose a module it requires (no readable manifest: no check).
        let manifest = std::fs::read(home.path.join("manifest.json"))
            .ok()
            .and_then(|b| Manifest::parse(&b).ok());
        let stays = state
            .modules
            .iter()
            .filter_map(|m| manifest.as_ref()?.module(m));
        let blocked: BTreeMap<&String, &String> = stays
            .filter(|m| !names.contains(&m.name))
            .flat_map(|m| {
                m.requires
                    .iter()
                    .filter(|r| names.contains(r))
                    .map(|r| (r, &m.name))
            })
            .collect();
        if !blocked.is_empty() {
            let lines: Vec<_> = (blocked.iter())
                .map(|(a, b)| format!("cannot remove {a}: {b} requires it"))
                .collect();
            bail!("{}", lines.join("\nkulman: "));
        }
    }
    // Removing everything leaves wiki's name in the agent configs of set-up projects.
    let projects = (names.is_none() && state.modules.iter().any(|m| m == "wiki")).then_some(
        "projects set up with kulman still name \"wiki\" in their agent configs: \
         run \"kulman setup --scope repo --remove\" (or local) in each first",
    );
    if ask {
        if let Some(line) = projects {
            println!("{line}");
        }
        let what = match &names {
            Some(names) => names.join(", "),
            None => (state.modules.iter().map(String::as_str))
                .chain(["kulman itself"])
                .collect::<Vec<_>>()
                .join(", "),
        };
        let question = format!("Remove {what} from {}?", home.path.display());
        if !ask_yes(&question, false)? {
            println!("nothing changed");
            return Ok(());
        }
    }
    // Stale temp entries are cleaned only in a home a state records as kulman's.
    home.clean_tmp()?;
    let Some(names) = names else {
        store::remove_all(home, &state.modules)?;
        for name in state.modules.iter().map(String::as_str).chain(["kulman"]) {
            println!("removed {name}");
        }
        if let (false, Some(line)) = (ask, projects) {
            println!("{line}");
        }
        return Ok(());
    };
    state.modules.retain(|m| !names.contains(m));
    store::remove_modules(home, &names, &state)?;
    for name in &names {
        println!("removed {name}");
    }
    Ok(())
}
