#![warn(clippy::wildcard_enum_match_arm)]
use plaz::setup::{self, Scope};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: plaz build [--source DIR] [--vault DIR] | plaz setup [--source DIR] --scope repo|local [--remove]";

/// The build's own stack: the flow walker recurses once per syntax tree
/// level. The OS reserves it lazily.
const STACK: usize = 512 << 20;

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let setup = match args.next().as_deref().and_then(OsStr::to_str) {
        Some("build") => false,
        Some("setup") => true,
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("--version") => {
            println!("plaz {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => return usage(),
    };
    let (mut source, mut vault) = (PathBuf::from("."), None);
    let (mut scope, mut remove) = (None, false);
    while let Some(flag) = args.next() {
        // `--source=DIR` needs a UTF-8 DIR; `--source DIR` takes any path.
        let Some(flag) = flag.to_str() else {
            return usage();
        };
        if matches!(flag, "--help" | "-h") {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        if setup && flag == "--remove" {
            remove = true;
            continue;
        }
        let (name, value) = match flag.split_once('=') {
            Some((name, v)) => (name, Some(v.into())),
            None => (flag, args.next()),
        };
        match (name, value) {
            ("--source", Some(v)) => source = v.into(),
            ("--vault", Some(v)) if !setup => vault = Some(PathBuf::from(v)),
            ("--scope", Some(v)) if setup => match v.to_str() {
                Some("repo") => scope = Some(Scope::Repo),
                Some("local") => scope = Some(Scope::Local),
                _ => return usage(),
            },
            _ => return usage(),
        }
    }
    if setup && scope.is_none() && !remove {
        return usage();
    }
    let vault = vault.unwrap_or_else(|| source.join(wiki_code::OUT_DIR));
    let run = move || -> Result<(), plaz::Error> {
        if setup && remove {
            let n = setup::remove(&vault)?;
            println!("map: {} removed{}", n.removed, alone(n.left_alone));
            return Ok(());
        }
        let n = plaz::build(&source, &vault)?;
        println!(
            "map: {} written, {} unchanged, {} removed{}",
            n.written,
            n.unchanged,
            n.removed,
            alone(n.left_alone)
        );
        if let Some(scope) = scope {
            if let Some(file) = setup::ensure_ignored(&source, scope)? {
                println!("ignore: {}", file.display());
            }
        }
        Ok(())
    };
    let done = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(run)
        .expect("a thread for the build")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("plaz: {e}");
            ExitCode::FAILURE
        }
    }
}

fn alone(n: usize) -> String {
    if n > 0 {
        format!(", {n} left alone")
    } else {
        String::new()
    }
}
