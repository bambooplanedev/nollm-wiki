#![warn(clippy::wildcard_enum_match_arm)]
//! Per-function decision flowcharts of a codebase, drawn without a model.

use std::path::{Path, PathBuf};

mod chart;
mod index;
pub mod names;
pub mod render;
pub mod setup;
pub mod write;

pub use wiki_code::links::{flow, model, resolve, source};
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
    let project = source::load(source).map_err(|_| Error::NotADirectory(source.to_path_buf()))?;
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
    let index = index::index(resolved.project(), &names);
    let mut pages = render::render(&resolved, &names, &links, &index.levels);
    pages.extend(index.pages);
    write::write(vault, &pages)
}
