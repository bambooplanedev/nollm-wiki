#![warn(clippy::wildcard_enum_match_arm)]
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: plaz build [--source DIR] [--vault DIR]";

/// The build's own stack: the flow walker recurses once per syntax tree
/// level. The OS reserves it lazily.
const STACK: usize = 512 << 20;

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    match args.next().as_deref().and_then(OsStr::to_str) {
        Some("build") => {}
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("--version") => {
            println!("plaz {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => return usage(),
    }
    let (mut source, mut vault) = (PathBuf::from("."), None);
    while let Some(flag) = args.next() {
        // `--source=DIR` needs a UTF-8 DIR; `--source DIR` takes any path.
        let Some(flag) = flag.to_str() else {
            return usage();
        };
        if matches!(flag, "--help" | "-h") {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        let (name, value) = match flag.split_once('=') {
            Some((name, v)) => (name, Some(PathBuf::from(v))),
            None => (flag, args.next().map(PathBuf::from)),
        };
        match (name, value) {
            ("--source", Some(v)) => source = v,
            ("--vault", Some(v)) => vault = Some(v),
            _ => return usage(),
        }
    }
    let vault = vault.unwrap_or_else(|| source.join(wiki_code::OUT_DIR));
    let built = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || plaz::build(&source, &vault))
        .expect("a thread for the build")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    match built {
        Ok(n) => {
            let alone = if n.left_alone > 0 {
                format!(", {} left alone", n.left_alone)
            } else {
                String::new()
            };
            println!(
                "map: {} written, {} unchanged, {} removed{alone}",
                n.written, n.unchanged, n.removed
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("plaz: {e}");
            ExitCode::FAILURE
        }
    }
}
