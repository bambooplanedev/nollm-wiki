//! kulman: installs, updates and sets up the modules of the kulman set into `~/.kulman`.

mod release;
mod store;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{ColorChoice, Parser, Subcommand};

use release::Manifest;
use store::{Home, State};

/// Default release location; `KULMAN_RELEASE_URL` overrides it.
const RELEASES: &str = "https://github.com/bambooplanedev/kulman/releases";

#[derive(Parser)]
#[command(version, about, color = ColorChoice::Never)]
struct Cli {
    /// With no command: what is installed, and whether a newer release exists.
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install modules of a release into ~/.kulman/bin (with kulman itself).
    Install {
        /// Modules to install, comma separated (default: the release's defaults).
        #[arg(long, value_delimiter = ',')]
        modules: Option<Vec<String>>,
        /// Install this release instead of the latest one.
        #[arg(long, value_name = "X.Y.Z", value_parser = parse_version)]
        version: Option<String>,
        /// Ask nothing; the release's defaults apply.
        #[arg(long)]
        yes: bool,
    },
    /// Update kulman and every installed module to the latest release.
    Update {
        /// Ask nothing.
        #[arg(long)]
        yes: bool,
    },
    /// Remove the named modules, or everything kulman installed.
    Remove {
        /// Modules to remove, comma separated (default: every module, kulman and its files).
        #[arg(long, value_delimiter = ',')]
        modules: Option<Vec<String>>,
        /// Ask nothing.
        #[arg(long)]
        yes: bool,
    },
}

fn parse_version(v: &str) -> Result<String, String> {
    if release::is_version(v) {
        Ok(v.to_string())
    } else {
        Err("expected X.Y.Z".to_string())
    }
}

/// A usage error: exit status 2.
#[derive(Debug)]
struct Usage(String);

impl std::fmt::Display for Usage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Usage {}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // A terminal behaves as --yes until the prompts exist.
    let result = match cli.command {
        None => status(),
        Some(Cmd::Install {
            modules,
            version,
            yes: _,
        }) => install(modules, version.as_deref()),
        Some(Cmd::Update { yes: _ }) => update(),
        Some(Cmd::Remove { modules, yes: _ }) => remove(modules),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kulman: {e:#}");
            ExitCode::from(if e.is::<Usage>() { 2 } else { 1 })
        }
    }
}

/// The release base, checked: a usage error when it is neither https:// nor file://.
fn release_base() -> Result<String> {
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
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        _ => bail!("no build for {OS}/{ARCH}"),
    })
}

/// `wanted`, or the manifest's defaults, plus everything they require.
fn choose(manifest: &Manifest, wanted: Option<Vec<String>>) -> Result<BTreeSet<String>> {
    let known: Vec<&str> = manifest.modules.iter().map(|m| m.name.as_str()).collect();
    let mut todo = match wanted {
        Some(names) => {
            if let Some(name) = names.iter().find(|n| !known.contains(&n.as_str())) {
                let known = known.join(", ");
                return Err(Usage(format!("unknown module {name}; known modules: {known}")).into());
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
        let Some(module) = manifest.modules.iter().find(|m| m.name == name) else {
            bail!("a module requires {name}, which is not in the release");
        };
        if chosen.insert(name) {
            todo.extend(module.requires.iter().cloned());
        }
    }
    Ok(chosen)
}

fn install(wanted: Option<Vec<String>>, version: Option<&str>) -> Result<()> {
    let base = release_base()?;
    let home_path = store::home_path()?;
    let target = target()?;
    for name in wanted.iter().flatten() {
        release::check_name(name, "--modules")?;
    }
    let (manifest, raw) = release::manifest(&base, version)?;
    let chosen = choose(&manifest, wanted)?;
    let home = Home::open(home_path)?;
    let result = place(&home, &base, &manifest, &raw, target, chosen);
    if result.is_err() {
        home.abandon();
    }
    result
}

/// Under the lock: checks the installed state, stages every binary, and only when all are
/// ready commits them.
fn place(
    home: &Home,
    base: &str,
    manifest: &Manifest,
    raw: &[u8],
    target: &str,
    chosen: BTreeSet<String>,
) -> Result<()> {
    let mut modules = BTreeSet::new();
    if let Some(state) = State::load(&home.path)? {
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
        modules.extend(state.modules);
    }
    // kulman last, so it is renamed into bin/ last.
    let mut names: Vec<String> = chosen.difference(&modules).cloned().collect();
    names.push("kulman".to_string());
    let staged = store::stage(home, base, manifest, target, &names)?;
    modules.extend(chosen);
    let state = State {
        version: manifest.version.clone(),
        modules: modules.into_iter().collect(),
    };
    store::commit(home, &staged, raw, &state)?;
    for s in &staged {
        println!("installed {} {}", s.name, manifest.version);
    }
    let bin = home.path.join("bin");
    let on_path =
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == bin));
    if !on_path {
        println!("add to PATH: {}", bin.display());
    }
    Ok(())
}

/// `kulman` with no command. Only reads: home is not created, nor locked.
fn status() -> Result<()> {
    let home = store::home_path()?;
    let Some(state) = State::load(&home)? else {
        println!("nothing installed; run kulman install");
        return Ok(());
    };
    println!("kulman {} in {}", state.version, home.display());
    for name in &state.modules {
        println!("{name}");
    }
    match release_base().and_then(|base| release::manifest(&base, None)) {
        Ok((latest, _)) => {
            if release::version_key(&latest.version) > release::version_key(&state.version) {
                println!("newer release: {} (kulman update)", latest.version);
            }
        }
        Err(e) => println!("could not check for a newer release: {e:#}"),
    }
    Ok(())
}

/// `state.json` exists under `home`: checked before anything is created.
fn is_installed(home: &Path) -> Result<bool> {
    home.join("state.json")
        .try_exists()
        .context("cannot read state.json")
}

fn update() -> Result<()> {
    let base = release_base()?;
    let home_path = store::home_path()?;
    let target = target()?;
    if !is_installed(&home_path)? {
        bail!("nothing installed; run kulman install");
    }
    let (manifest, raw) = release::manifest(&base, None)?;
    let home = Home::open(home_path)?;
    let result = renew(&home, &base, &manifest, &raw, target);
    if result.is_err() {
        home.abandon();
    }
    result
}

/// Under the lock: unless already current or newer, stages kulman and every installed module
/// at the latest version, and only when all are ready commits them (kulman last).
fn renew(home: &Home, base: &str, manifest: &Manifest, raw: &[u8], target: &str) -> Result<()> {
    let Some(state) = State::load(&home.path)? else {
        bail!("nothing installed; run kulman install");
    };
    let (have, latest) = (&state.version, &manifest.version);
    // The installed modules, plus what they require in the latest release.
    for name in &state.modules {
        manifest.archive(name, target)?;
    }
    let modules = choose(manifest, Some(state.modules.clone()))?;
    let bin = home.path.join("bin");
    let mut names: Vec<String> = modules.iter().cloned().collect();
    names.push("kulman".to_string());
    match release::version_key(have).cmp(&release::version_key(latest)) {
        std::cmp::Ordering::Greater => {
            println!("installed {have} is newer than the latest release {latest}; nothing changed");
            return Ok(());
        }
        std::cmp::Ordering::Equal if names.iter().all(|n| bin.join(n).is_file()) => {
            println!("up to date");
            return Ok(());
        }
        _ => {}
    }
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

fn remove(names: Option<Vec<String>>) -> Result<()> {
    for name in names.iter().flatten() {
        release::check_name(name, "--modules")?;
        if name == "kulman" {
            let why = "kulman is not a module; kulman remove without --modules removes everything";
            return Err(Usage(why.to_string()).into());
        }
    }
    let home_path = store::home_path()?;
    // Without state.json, a home with kulman's `.lock` is left by a remove killed before its
    // last step: it is finished below. Anything else is not opened, so nothing is created.
    if !is_installed(&home_path)? && !home_path.join(".lock").try_exists()? {
        println!("nothing installed");
        return Ok(());
    }
    let home = Home::open(home_path)?;
    let result = uninstall(&home, names);
    // A run that created home (it vanished while this run waited) found nothing: undo it.
    if result.is_err() || home.created {
        home.abandon();
    }
    result
}

/// Under the lock: every name is checked (by `State::load` and against the state) before the
/// first file is deleted.
fn uninstall(home: &Home, names: Option<Vec<String>>) -> Result<()> {
    let Some(mut state) = State::load(&home.path)? else {
        store::remove_all(home, &[])?;
        println!("nothing installed");
        return Ok(());
    };
    let Some(mut names) = names else {
        store::remove_all(home, &state.modules)?;
        for name in state.modules.iter().map(String::as_str).chain(["kulman"]) {
            println!("removed {name}");
        }
        return Ok(());
    };
    names.sort();
    names.dedup();
    if let Some(name) = names.iter().find(|n| !state.modules.contains(n)) {
        bail!("{name} is not installed");
    }
    state.modules.retain(|m| !names.contains(m));
    store::remove_modules(home, &names, &state)?;
    for name in &names {
        println!("removed {name}");
    }
    Ok(())
}
