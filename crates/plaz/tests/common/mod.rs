#![allow(dead_code, reason = "each test file uses a different subset")]

use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Writes `files` under `dir`, creating directories.
pub fn write_files(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let f = dir.join(path);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    }
}

/// Writes `files` under `<tmp>/src-root`, builds into `<tmp>/vault` and
/// returns the vault.
pub fn built(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    write_files(&src, files);
    plaz::build(&src, &vault).unwrap();
    (d, vault)
}

/// The page `id` (like `"map/src/a_rs"`) of `vault`.
pub fn map_page(vault: &Path, id: &str) -> String {
    let path = vault.join(format!("{id}.md"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The `- ` lines under `**{label}**` inside `## {anchor}`, without the `- `.
pub fn fn_section(page: &str, anchor: &str, label: &str) -> Vec<String> {
    let heading = format!("## {anchor}");
    let label = format!("**{label}**");
    let section = page
        .lines()
        .skip_while(|l| *l != heading)
        .skip(1)
        .take_while(|l| !l.starts_with("## "));
    section
        .skip_while(|l| *l != label)
        .skip(1)
        .take_while(|l| l.starts_with("- "))
        .map(|l| l[2..].to_string())
        .collect()
}

/// The whole `## {anchor}` section, heading included, up to the next `## `.
pub fn fn_block(page: &str, anchor: &str) -> String {
    let heading = format!("## {anchor}");
    let mut lines = page.lines().skip_while(|l| *l != heading);
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("no {heading} in\n{page}"));
    let rest: Vec<&str> = lines.take_while(|l| !l.starts_with("## ")).collect();
    format!("{first}\n{}\n", rest.join("\n"))
}

/// The headings (`## …`) of a page, in order.
pub fn headings(page: &str) -> Vec<&str> {
    page.lines().filter_map(|l| l.strip_prefix("## ")).collect()
}

/// The lines of the index page `id` under `## {heading}`, up to the next
/// `## `, blank lines left out.
pub fn index_section(vault: &Path, id: &str, heading: &str) -> Vec<String> {
    let page = map_page(vault, id);
    let h = format!("## {heading}");
    let lines = page.lines().skip_while(|l| *l != h).skip(1);
    let lines = lines.take_while(|l| !l.starts_with("## "));
    lines
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}
