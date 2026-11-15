//! kulman: installs, updates and sets up the modules of the kulman set into `~/.kulman-home`.

mod install;
mod prompt;
mod release;
mod remove;
mod setup;
mod status;
mod store;

use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, ColorChoice, Parser, Subcommand};

use install::{install, update};
use remove::remove;
use setup::setup;
use status::status;

#[derive(Parser)]
#[command(version, about, color = ColorChoice::Never)]
struct Cli {
    /// With no command, shows status: what is installed, and whether a newer release exists.
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show what is installed and whether a newer release exists (what no command shows).
    Status,
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
        None | Some(Cmd::Status) => status(),
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

/// `dir` is on this process's `PATH`, by real path: an entry reaching it through a symlink counts.
/// A relative entry, or one that cannot be resolved (and every entry, when `dir` cannot), is
/// compared as text.
fn on_path(dir: &Path) -> bool {
    let real = dir.canonicalize().ok();
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| {
            d == dir
                || real.as_ref().is_some_and(|real| {
                    d.is_absolute() && d.canonicalize().is_ok_and(|d| d == *real)
                })
        })
    })
}

/// What kulman says when `bin` is not on `PATH`: two lines, the second one to copy.
fn path_hint(bin: &Path) -> String {
    let bin = bin.display();
    format!("{bin} is not on PATH; add it:\n    export PATH=\"{bin}:$PATH\"")
}

/// `state.json` exists under `home`: checked before anything is created.
fn is_installed(home: &Path) -> Result<bool> {
    home.join("state.json")
        .try_exists()
        .context("cannot read state.json")
}
