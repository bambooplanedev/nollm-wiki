//! kulman: installs, updates and sets up the modules of the kulman set into `~/.kulman`.

mod release;
mod store;

use std::collections::BTreeSet;
use std::process::ExitCode;

use anyhow::{bail, Result};
use clap::{ColorChoice, Parser, Subcommand};

use release::Manifest;
use store::{Home, State};

/// Default release location; `KULMAN_RELEASE_URL` overrides it.
const RELEASES: &str = "https://github.com/bambooplanedev/kulman/releases";

#[derive(Parser)]
#[command(version, about, color = ColorChoice::Never)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
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
    let result = match cli.command {
        // A terminal behaves as --yes until the prompts exist.
        Cmd::Install {
            modules,
            version,
            yes: _,
        } => install(modules, version.as_deref()),
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
    release::check_base(&base).map_err(|e| Usage(e.to_string()))?;
    Ok(base.trim_end_matches('/').to_string())
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
    if let Some(state) = State::load(home)? {
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
