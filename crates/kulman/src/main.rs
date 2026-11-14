//! kulman: installs, updates and sets up the modules of the kulman set into `~/.kulman`.

mod prompt;
mod release;
mod store;

use std::collections::BTreeSet;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::{Command, ExitCode};

use anyhow::{bail, Context, Result};
use clap::{Args, ColorChoice, Parser, Subcommand};

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
    /// Set up the current project: runs each chosen module's own setup.
    Setup(SetupArgs),
}

#[derive(Args)]
#[allow(clippy::struct_excessive_bools)] // independent switches, passed through
struct SetupArgs {
    /// Modules to set up, comma separated (default: every installed module).
    #[arg(long, value_delimiter = ',')]
    modules: Option<Vec<String>>,
    /// kalka: agent memory on (default: the release's choice, on unless it says off).
    #[arg(long, conflicts_with = "no_memory")]
    memory: bool,
    /// kalka: agent memory off.
    #[arg(long)]
    no_memory: bool,
    /// kalka: the agents to wire, passed through as given.
    #[arg(long, value_name = "LIST")]
    agent: Option<String>,
    /// repo (committed, for the whole team) or local (this checkout only).
    #[arg(long, value_parser = ["repo", "local"])]
    scope: Option<String>,
    /// kalka: no hook.
    #[arg(long)]
    no_hook: bool,
    /// Undo each module's setup.
    #[arg(long)]
    remove: bool,
    /// Ask nothing; a value with no default must then be given as a flag.
    #[arg(long)]
    yes: bool,
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
        None => status(),
        Some(Cmd::Install {
            modules,
            version,
            yes,
        }) => install(modules, version.as_deref(), asks(yes)),
        Some(Cmd::Update { yes }) => update(asks(yes)),
        Some(Cmd::Remove { modules, yes }) => remove(modules, asks(yes)),
        Some(Cmd::Setup(args)) => setup(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kulman: {e:#}");
            ExitCode::from(if e.is::<Usage>() { 2 } else { 1 })
        }
    }
}

/// Questions are asked only when stdin and stdout are terminals and `--yes` is absent.
fn asks(yes: bool) -> bool {
    !yes && io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// `prompt::confirm` at the terminal.
fn ask_yes(question: &str, default_yes: bool) -> Result<bool> {
    Ok(prompt::confirm(
        &mut io::stdin().lock(),
        &mut io::stdout(),
        question,
        default_yes,
    )?)
}

/// `prompt::choose` at the terminal.
fn ask_choose(title: &str, items: &[(&str, &str, bool)], one: bool) -> Result<Vec<String>> {
    Ok(prompt::choose(
        &mut io::stdin().lock(),
        &mut io::stdout(),
        title,
        items,
        one,
    )?)
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

fn install(wanted: Option<Vec<String>>, version: Option<&str>, ask: bool) -> Result<()> {
    let base = release_base()?;
    let home_path = store::home_path()?;
    let target = target()?;
    for name in wanted.iter().flatten() {
        release::check_name(name, "--modules")?;
    }
    // Installed at V with no --version: V's manifest, so a newer release does not block adding
    // modules; that one is only announced. The lock re-checks the version.
    let installed = match version {
        None => State::load(&home_path)?.map(|s| s.version),
        Some(_) => None,
    };
    let (manifest, raw, newer) = match installed {
        None => {
            let (manifest, raw) = release::manifest(&base, version)?;
            (manifest, raw, None)
        }
        Some(v) => match release::manifest(&base, None) {
            // Not newer: used as it is (an older one is refused under the lock).
            Ok((m, raw)) if release::version_key(&m.version) <= release::version_key(&v) => {
                (m, raw, None)
            }
            latest => {
                let newer = latest.ok().map(|(m, _)| m.version);
                let (manifest, raw) = release::manifest(&base, Some(&v))?;
                (manifest, raw, newer)
            }
        },
    };
    let wanted = match wanted {
        None if ask => {
            let items: Vec<_> = (manifest.modules.iter())
                .map(|m| (m.name.as_str(), m.summary.as_str(), m.default))
                .collect();
            Some(ask_choose("Modules to install:", &items, false)?)
        }
        wanted => wanted,
    };
    let chosen = choose(&manifest, wanted)?;
    if ask {
        let bin = home_path.join("bin");
        let question = format!("Install {} module(s) into {}?", chosen.len(), bin.display());
        if !ask_yes(&question, true)? {
            println!("nothing changed");
            return Ok(());
        }
    }
    let home = Home::open(home_path)?;
    let result = place(&home, &base, &manifest, &raw, target, chosen);
    if result.is_err() {
        home.abandon();
        return result;
    }
    if let Some(newer) = newer {
        println!("newer release {newer}: kulman update");
    }
    let bin = home.path.join("bin");
    let on_path =
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == bin));
    if !on_path {
        println!("add to PATH: {}", bin.display());
    }
    Ok(())
}

/// Under the lock: checks the installed state, stages every chosen module not installed and
/// every listed binary missing from `bin/`, and only when all are ready commits them.
fn place(
    home: &Home,
    base: &str,
    manifest: &Manifest,
    raw: &[u8],
    target: &str,
    chosen: BTreeSet<String>,
) -> Result<()> {
    home.clean_tmp()?;
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
    let bin = home.path.join("bin");
    let missing = |name: &String| !bin.join(name).is_file();
    // kulman last, so it is renamed into bin/ last.
    let mut names: Vec<String> = chosen
        .union(&modules)
        .filter(|n| missing(n))
        .cloned()
        .collect();
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
        if home.join("bin").join(name).is_file() {
            println!("{name}");
        } else {
            println!("{name}  (missing: run kulman update)");
        }
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

fn update(ask: bool) -> Result<()> {
    let base = release_base()?;
    let home_path = store::home_path()?;
    let target = target()?;
    if !is_installed(&home_path)? {
        bail!("nothing installed; run kulman install");
    }
    let (manifest, raw) = release::manifest(&base, None)?;
    if ask {
        // Asked only when there is a newer release; the answer is checked again under the lock.
        if let Some(state) = State::load(&home_path)? {
            let (have, latest) = (&state.version, &manifest.version);
            if release::version_key(latest) > release::version_key(have) {
                let question = format!("Update {} module(s) to {latest}?", state.modules.len());
                if !ask_yes(&question, true)? {
                    println!("nothing changed");
                    return Ok(());
                }
            }
        }
    }
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
    home.clean_tmp()?;
    let Some(state) = State::load(&home.path)? else {
        bail!("nothing installed; run kulman install");
    };
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

fn remove(names: Option<Vec<String>>, ask: bool) -> Result<()> {
    for name in names.iter().flatten() {
        release::check_name(name, "--modules")?;
        if name == "kulman" {
            let why = "kulman is not a module; kulman remove without --modules removes everything";
            return Err(Usage(why.to_string()).into());
        }
    }
    let home_path = store::home_path()?;
    // Without state.json, a home with a `.lock` may be left by a remove killed before its last
    // step: only that lock is removed below. Anything else is not opened, so nothing is created.
    if !is_installed(&home_path)? && !home_path.join(".lock").try_exists()? {
        println!("nothing installed");
        return Ok(());
    }
    if ask {
        // With no state only the lock is removed: nothing to ask.
        if let Some(state) = State::load(&home_path)? {
            let what = match &names {
                Some(names) => names.join(", "),
                None => (state.modules.iter().map(String::as_str))
                    .chain(["kulman itself"])
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            let question = format!("Remove {what} from {}?", home_path.display());
            if !ask_yes(&question, false)? {
                println!("nothing changed");
                return Ok(());
            }
        }
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
        // No record of owning anything: no binary and no manifest is deleted.
        let holds = store::remove_lock(home)?;
        println!("nothing installed");
        if holds {
            let home = home.path.display();
            println!("{home} holds files kulman has no record of; left as they are");
        }
        return Ok(());
    };
    // Stale temp entries are cleaned only in a home a state records as kulman's.
    home.clean_tmp()?;
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

/// `kulman setup`: runs `<home>/bin/<module> setup …` for each chosen module, in manifest order,
/// in the current directory, and stops at the first failure with its exit status.
fn setup(a: SetupArgs) -> Result<()> {
    for name in a.modules.iter().flatten() {
        release::check_name(name, "--modules")?;
        if name == "kulman" {
            return Err(Usage("kulman is not a module".to_string()).into());
        }
    }
    let home = store::home_path()?;
    let Some(state) = State::load(&home)? else {
        bail!("nothing installed; run kulman install");
    };
    let manifest = std::fs::read(home.join("manifest.json"))
        .context("cannot read manifest.json")
        .and_then(|bytes| Manifest::parse(&bytes))?;
    if let Some(name) = a
        .modules
        .iter()
        .flatten()
        .find(|n| !state.modules.contains(n))
    {
        bail!("{name} is not installed; run kulman install --modules {name}");
    }
    let ask = asks(a.yes);
    let module = |name: &str| manifest.modules.iter().find(|m| m.name == name);
    let mut installed = state.modules.clone();
    // Manifest order; any the manifest lacks last.
    installed.sort_by_key(|n| {
        let at = manifest.modules.iter().position(|m| &m.name == n);
        at.unwrap_or(usize::MAX)
    });
    let wanted = match a.modules {
        Some(wanted) => wanted,
        None if ask => {
            let items: Vec<_> = (installed.iter())
                .map(|n| {
                    (
                        n.as_str(),
                        module(n).map_or("", |m| m.summary.as_str()),
                        true,
                    )
                })
                .collect();
            ask_choose("Modules to set up:", &items, false)?
        }
        None => installed.clone(),
    };
    let chosen: Vec<&String> = installed.iter().filter(|n| wanted.contains(n)).collect();
    let kalka = chosen.iter().any(|n| *n == "kalka");
    for (given, flag) in [
        (a.memory, "--memory"),
        (a.no_memory, "--no-memory"),
        (a.agent.is_some(), "--agent"),
        (a.no_hook, "--no-hook"),
    ] {
        if given && !kalka {
            let why = format!("{flag} applies to kalka, which is not chosen");
            return Err(Usage(why).into());
        }
    }
    let memory_default = module("kalka")
        .and_then(|m| m.options.iter().find(|o| o.name == "memory"))
        .is_none_or(|o| o.default);
    let memory = match (a.memory, a.no_memory) {
        (true, _) => true,
        (_, true) => false,
        _ if kalka && ask && !a.remove => ask_yes("agent memory?", memory_default)?,
        _ => memory_default,
    };
    // kalka needs the scope even to remove; plaz does not.
    let scope_needed = kalka || !a.remove;
    let scope = match a.scope {
        None if scope_needed && ask => {
            let items = [
                ("repo", "committed, for the whole team", false),
                ("local", "this checkout only, not committed", false),
            ];
            ask_choose("Scope:", &items, true)?.pop()
        }
        scope => scope,
    };
    if scope.is_none() && scope_needed {
        return Err(Usage("--scope is required (repo or local)".to_string()).into());
    }
    let bin = home.join("bin");
    for name in &chosen {
        let path = bin.join(name);
        if !path.is_file() {
            let path = path.display();
            bail!("{name} is installed but {path} is missing; run kulman update");
        }
    }
    for name in chosen {
        let is_kalka = name == "kalka";
        let mut args = vec!["setup"];
        if let (true, Some(agent)) = (is_kalka, &a.agent) {
            args.extend(["--agent", agent]);
        }
        if let Some(scope) = &scope {
            args.extend(["--scope", scope]);
        }
        if is_kalka && a.no_hook {
            args.push("--no-hook");
        }
        if is_kalka {
            args.push(if memory { "--memory" } else { "--no-memory" });
        }
        if a.remove {
            args.push("--remove");
        }
        let path = bin.join(name);
        let status = Command::new(&path)
            .args(&args)
            .status()
            .with_context(|| format!("cannot run {}", path.display()))?;
        if !status.success() {
            eprintln!("kulman: {name} setup failed");
            // Its own exit status; a module ended by a signal has none: 1.
            std::process::exit(status.code().unwrap_or(1));
        }
    }
    Ok(())
}
