//! `kulman` with no command: what is installed, and whether a newer release exists.

use anyhow::Result;

use crate::install::release_base;
use crate::{path_hint, reachable, release, store};
use store::State;

/// `kulman` with no command, the release check limited to `wait`. Only reads: home is not created, nor locked.
pub(super) fn status(wait: release::Wait) -> Result<()> {
    let home = store::home_path()?;
    let Some(state) = State::load(&home)? else {
        println!(
            "nothing installed in {}; run kulman install",
            home.display()
        );
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
    if !reachable(
        &bin,
        state.modules.iter().map(String::as_str).chain(["kulman"]),
    ) {
        println!("{}", path_hint(&bin));
    }
    // Said with the base and the first cause: the URL under the base adds nothing.
    let latest = release_base().and_then(|base| {
        release::manifest(&base, None, wait)
            .map_err(|e| anyhow::anyhow!("{base}: {}", e.root_cause()))
    });
    match latest {
        Ok((latest, _)) => {
            if release::version_key(&latest.version) > release::version_key(&state.version) {
                println!("newer release: {} (kulman update)", latest.version);
            } else {
                println!("up to date");
            }
        }
        Err(e) => println!("could not check for a newer release ({e:#})"),
    }
    Ok(())
}
