//! kulman: installs, updates and sets up the modules of the kulman set into `~/.kulman-home`.

mod prompt;
mod release;
mod store;

use std::collections::BTreeSet;
use std::io::{self, IsTerminal};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

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
    /// Install modules of a release into ~/.kulman-home/bin (with kulman itself).
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
    let changes = matches!(cli.command, Some(Cmd::Install { .. } | Cmd::Update { .. }));
    let result = match cli.command {
        None => status(),
        Some(Cmd::Install {
            modules,
            version,
            yes,
        }) => module_names(modules).and_then(|m| install(m, version.as_deref(), asks(yes))),
        Some(Cmd::Update { yes }) => update(asks(yes)),
        Some(Cmd::Remove { modules, yes }) => module_names(modules).and_then(|m| remove(m, yes)),
        Some(Cmd::Setup(mut args)) => module_names(args.modules.take()).and_then(|m| {
            args.modules = m;
            setup(args)
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kulman: {e:#}");
            if changes && !store::COMMITTING.load(std::sync::atomic::Ordering::Relaxed) {
                eprintln!("nothing changed");
            }
            ExitCode::from(if e.is::<Usage>() { 2 } else { 1 })
        }
    }
}

/// `--modules`, each name trimmed: a usage error for one that is not a module name, checked
/// before any path or URL is built.
fn module_names(names: Option<Vec<String>>) -> Result<Option<Vec<String>>> {
    let check = |name: String| {
        let name = name.trim();
        if release::is_name(name) {
            Ok(name.to_string())
        } else {
            Err(Usage(format!("--modules: bad name {name:?}")).into())
        }
    };
    names
        .map(|names| names.into_iter().map(check).collect())
        .transpose()
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

/// `dir` is on this process's `PATH`.
fn on_path(dir: &Path) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == dir))
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

/// The usage error for a `--modules` name the release does not have.
fn unknown_module(manifest: &Manifest, name: &str) -> anyhow::Error {
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

fn install(wanted: Option<Vec<String>>, version: Option<&str>, ask: bool) -> Result<()> {
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
/// ready commits them. With `ask`, asks first; `false` when declined.
fn place(
    home: &Home,
    base: &str,
    wanted: Option<Vec<String>>,
    version: Option<&str>,
    target: &str,
    ask: bool,
) -> Result<bool> {
    home.clean_tmp()?;
    let state = State::load(&home.path)?;
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
    let bin = home.join("bin");
    if !on_path(&bin) {
        println!("PATH: {} is not on PATH (add it)", bin.display());
    }
    // Said with the base and the first cause: the URL under the base adds nothing.
    let latest = release_base().and_then(|base| {
        release::manifest(&base, None, release::STATUS_WAIT)
            .map_err(|e| anyhow::anyhow!("{base}: {}", e.root_cause()))
    });
    match latest {
        Ok((latest, _)) => {
            if release::version_key(&latest.version) > release::version_key(&state.version) {
                println!("newer release: {} (kulman update)", latest.version);
            }
        }
        Err(e) => println!("could not check for a newer release ({e:#})"),
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

fn remove(names: Option<Vec<String>>, yes: bool) -> Result<()> {
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
    }
    // Removing everything leaves kalka's name in the agent configs of set-up projects.
    let projects = (names.is_none() && state.modules.iter().any(|m| m == "kalka")).then_some(
        "projects set up with kulman still name \"kalka\" in their agent configs: \
         run kulman setup --remove in each first",
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

/// `kulman setup`: runs `<home>/bin/<module> setup …` for each chosen module, in manifest order,
/// in the current directory, whatever the earlier ones returned; when any failed, names them
/// and exits with the first failure's status.
fn setup(a: SetupArgs) -> Result<()> {
    for name in a.modules.iter().flatten() {
        if name == "kulman" {
            return Err(Usage("kulman is not a module".to_string()).into());
        }
    }
    let home = store::home_path()?;
    let Some(state) = State::load(&home)? else {
        bail!("nothing installed; run kulman install");
    };
    if state.modules.is_empty() {
        bail!("nothing to set up: no module is installed (kulman install)");
    }
    // The modules write into the project's `.kulman`: never into kulman's own home.
    let project = std::env::current_dir().and_then(|d| std::fs::metadata(d.join(".kulman")));
    if let (Ok(p), Ok(h)) = (project, std::fs::metadata(&home)) {
        if (p.dev(), p.ino()) == (h.dev(), h.ino()) {
            bail!(
                "{} is kulman's own folder; set the project up from another directory or move \
                 KULMAN_HOME",
                home.display()
            );
        }
    }
    let manifest = std::fs::read(home.join("manifest.json"))
        .context("cannot read manifest.json")
        .and_then(|bytes| Manifest::parse(&bytes))?;
    let module = |name: &str| manifest.module(name);
    for name in a.modules.iter().flatten() {
        if module(name).is_none() {
            return Err(unknown_module(&manifest, name));
        }
        if !state.modules.contains(name) {
            bail!("{name} is not installed; run kulman install --modules {name}");
        }
    }
    let ask = asks(a.yes);
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
        let meta = std::fs::metadata(&path)
            .ok()
            .filter(std::fs::Metadata::is_file);
        match meta.map(|m| m.permissions().mode()) {
            None => bail!(
                "{name} is installed but {} is missing; run kulman update",
                path.display()
            ),
            Some(mode) if mode & 0o111 == 0 => bail!("{} is not executable", path.display()),
            Some(_) => {}
        }
    }
    // The modules find each other (kalka its own name) on PATH: `bin/` first when it is not on
    // it, in their environment only; one that cannot be on a PATH (a `:` in it) leaves PATH as
    // it is.
    let off_path = !on_path(&bin);
    let own = std::env::var_os("PATH").unwrap_or_default();
    let modules_path = if off_path {
        std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&own))).ok()
    } else {
        None
    };
    if off_path && kalka && !a.remove {
        let bin = bin.display();
        eprintln!("kulman: {bin} is not on PATH");
        eprintln!(
            "kulman: the agent configs written now start \"kalka\"; an agent started without it \
             on PATH will not find it"
        );
        eprintln!("kulman: add to PATH: {bin}");
    }
    // Names of the modules that failed, and the first failure's status.
    let mut failures: Vec<&str> = Vec::new();
    let mut first_code = None;
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
        // Not asking: a module's own questions get no input either.
        let input = if ask { Stdio::inherit() } else { Stdio::null() };
        let mut command = Command::new(&path);
        if let Some(p) = &modules_path {
            command.env("PATH", p);
        }
        // Its own exit status; a module ended by a signal has none: 1.
        let (how, code) = match command.args(&args).stdin(input).status() {
            Ok(status) if status.success() => continue,
            Ok(status) => match (status.code(), status.signal()) {
                (Some(code), _) => (format!("exit {code}"), code),
                (None, signal) => (format!("signal {}", signal.unwrap_or(0)), 1),
            },
            Err(e) => (format!("cannot run {}: {e}", path.display()), 1),
        };
        eprintln!("kulman: {name} setup failed ({how})");
        failures.push(name);
        first_code.get_or_insert(code);
    }
    if let Some(code) = first_code {
        eprintln!(
            "kulman: setup finished with errors: {}",
            failures.join(", ")
        );
        std::process::exit(code);
    }
    Ok(())
}
