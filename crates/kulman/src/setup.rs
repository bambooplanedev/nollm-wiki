//! `kulman setup`: runs each chosen module's own setup in the current project.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::install::unknown_module;
use crate::release::Manifest;
use crate::{ask_choose, ask_yes, asks, on_path, store, SetupArgs, Usage};
use store::State;

/// `kulman setup`: runs `<home>/bin/<module> setup …` for each chosen module, in manifest order,
/// in the current directory, whatever the earlier ones returned; when any failed, names them
/// and exits with the first failure's status.
pub(super) fn setup(a: SetupArgs) -> Result<()> {
    for name in a.modules.iter().flatten() {
        if name == "kulman" {
            return Err(Usage("kulman is not a module".to_string()).into());
        }
    }
    let home = store::home_path()?;
    let Some(state) = State::load(&home)? else {
        bail!(
            "nothing installed in {}; run kulman install",
            home.display()
        );
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
