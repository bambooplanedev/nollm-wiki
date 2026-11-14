//! The source stage: files in, an analysed `Project` out.

use crate::flow;
use crate::model::{File, Project};
use crate::Error;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use wiki_code::index::Family;
use wiki_code::walk::walk;
use wiki_code::{analyze, handles, pair_declarations};

/// `(relative path, text)` pairs in, `Project` out. A code file the parser
/// fails on is not in `files`; it is listed in `unreadable`. Other non-code
/// paths are in neither.
pub fn project(sources: impl IntoIterator<Item = (String, String)>) -> Project {
    let mut files = BTreeMap::new();
    let mut unreadable = Vec::new();
    let mut crates = BTreeSet::new();
    for (path, text) in sources {
        if manifest(&path) {
            let dir = path.trim_end_matches("Cargo.toml").trim_end_matches('/');
            crates.extend(crate_names(&text).into_iter().map(|n| (n, dir.to_string())));
            continue;
        }
        let Some(a) = analyze(&path, &text) else {
            if handles(&path) {
                unreadable.push(path);
            }
            continue;
        };
        let family = Family::of(Some(&a.file.lang));
        let python = match family {
            Family::Python => true,
            Family::Rust | Family::Cpp | Family::Go | Family::Shell | Family::Ecma => false,
        };
        let mut file = File {
            imports: if python {
                flow::module_imports(&a)
            } else {
                Vec::new()
            },
            module_code: python && flow::module_code(&a),
            test_fns: 0,
            family,
            syntax_errors: a.sources.iter().any(|s| s.tree().root_node().has_error()),
            lines: text.lines().count().max(1),
            code: a.file.clone(),
            flows: BTreeMap::new(),
        };
        flow::attach(&mut file, &a);
        files.insert(path, file);
    }
    pair_declarations(&mut files);
    unreadable.sort();
    Project {
        files,
        unreadable,
        crates,
    }
}

fn manifest(path: &str) -> bool {
    path == "Cargo.toml" || path.ends_with("/Cargo.toml")
}

/// The `name = "…"` values under `[package]` and `[lib]`, `-` read as `_`:
/// none for a virtual workspace or a `[package]` with no literal name.
fn crate_names(manifest: &str) -> Vec<String> {
    let (mut section, mut names) = ("", Vec::new());
    for line in manifest.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[') {
            section = header.split(']').next().unwrap_or("").trim();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let quoted = value.trim_start().strip_prefix(['"', '\'']);
        let name = quoted.and_then(|v| v.split(['"', '\'']).next());
        if let (Some(name), "name", "package" | "lib") = (name, key.trim(), section) {
            names.push(name.replace('-', "_"));
        }
    }
    names
}

pub fn load(root: &Path) -> Result<Project, Error> {
    if !root.is_dir() {
        return Err(Error::NotADirectory(root.to_path_buf()));
    }
    let walked = walk(root, true, &|p| handles(p) || manifest(p));
    let mut p = project(
        walked
            .files
            .into_iter()
            .map(|f| (f.rel_path, String::from_utf8_lossy(&f.bytes).into_owned())),
    );
    p.unreadable.extend(walked.unreadable);
    p.unreadable.sort();
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_is_pure_sorted_and_keeps_call_sites() {
        let p = project([
            ("src/b.rs".to_string(), "pub fn h() {}\n".to_string()),
            ("src/a.rs".to_string(), "fn f() { h(); }\n".to_string()),
        ]);
        assert_eq!(p.files.keys().collect::<Vec<_>>(), ["src/a.rs", "src/b.rs"]);
        let a = &p.files["src/a.rs"];
        assert_eq!((a.family, a.syntax_errors), (Family::Rust, false));
        assert_eq!(a.code.items[0].call_sites.len(), 1);
        assert_eq!(
            p,
            project([
                ("src/a.rs".to_string(), "fn f() { h(); }\n".to_string()),
                ("src/b.rs".to_string(), "pub fn h() {}\n".to_string()),
            ])
        );
    }

    #[test]
    fn crate_names_come_from_package_and_lib_a_nameless_package_is_no_crate() {
        let p = project([
            (
                "Cargo.toml".to_string(),
                "[workspace]\n[workspace.package]\nname = \"ws\"\n".to_string(),
            ),
            (
                "d/Cargo.toml".to_string(),
                "[package] # the crate\r\nversion.workspace = true\r\nname = \"crlf\"\r\n"
                    .to_string(),
            ),
            (
                "a/Cargo.toml".to_string(),
                "[package]\nversion = \"1\"\n[dependencies]\nname = \"x\"\n".to_string(),
            ),
            (
                "b/Cargo.toml".to_string(),
                "[package]\nname = 'b-c' # c\n[lib]\nname = \"bee\"\n[[bin]]\nname = \"no\"\n"
                    .to_string(),
            ),
        ]);
        assert!(p.unreadable.is_empty(), "{:?}", p.unreadable);
        let crates: Vec<_> = p
            .crates
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_str()))
            .collect();
        assert_eq!(crates, [("b_c", "b"), ("bee", "b"), ("crlf", "d")]);
    }

    #[test]
    fn a_non_code_path_is_in_neither_files_nor_unreadable() {
        let p = project([("README.md".to_string(), "x".to_string())]);
        assert!(p.files.is_empty() && p.unreadable.is_empty());
    }

    #[test]
    fn a_file_with_error_nodes_is_flagged_not_dropped() {
        let p = project([(
            "src/a.rs".to_string(),
            "fn ok() {}\nfn broken( { \n".to_string(),
        )]);
        assert!(p.files["src/a.rs"].syntax_errors);
    }

    #[test]
    fn load_skips_hidden_and_non_code_and_rejects_a_missing_root() {
        let d = tempfile::tempdir().unwrap();
        for (p, t) in [
            ("src/a.rs", "fn f() {}\n"),
            ("README.md", "x"),
            (".wiki/a.rs", "fn no() {}\n"),
        ] {
            let f = d.path().join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        }
        assert_eq!(
            load(d.path()).unwrap().files.keys().collect::<Vec<_>>(),
            ["src/a.rs"]
        );
        assert!(matches!(
            load(Path::new("/no/such/dir")),
            Err(Error::NotADirectory(_))
        ));
    }
}
