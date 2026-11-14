#![warn(clippy::wildcard_enum_match_arm)]
//! Per-function decision flowcharts of a codebase, drawn without a model.

use std::path::{Path, PathBuf};

mod chart;
pub mod flow;
pub mod model;
pub mod names;
pub mod render;
pub mod resolve;
pub mod source;
mod uses;
pub mod write;

pub use write::Written;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{} is not a directory", .0.display())]
    NotADirectory(PathBuf),
}

/// Build the map of `source` into `vault`, naming on stderr each code file
/// that could not be read. One vault holds the map of one source tree: a
/// tree with no code file leaves the vault as it is, with a warning, so a
/// wrong `source` does not wipe a map.
pub fn build(source: &Path, vault: &Path) -> Result<Written, Error> {
    let project = source::load(source)?;
    for path in &project.unreadable {
        eprintln!("warning: could not read {path}");
    }
    if project.files.is_empty() {
        eprintln!(
            "warning: no code file found in {}, the map is left as it is",
            source.display()
        );
        return Ok(Written::default());
    }
    let resolved = resolve::resolve(project);
    let names = names::names(&resolved);
    let links = write::Links::load(vault);
    write::write(vault, &render::render(&resolved, &names, &links))
}
