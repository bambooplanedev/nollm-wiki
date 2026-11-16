//! The source stage: files in, an analysed `Project` out.

mod cpp;

use crate::flow;
use crate::model::{File, NotDrawn, Project};
use crate::Error;
use cpp::cpp_facts;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use wiki_code::index::Family;
use wiki_code::model::CallVia;
use wiki_code::walk::walk;
use wiki_code::{analyze, handles, pair_declarations, Analyzed};

/// `(relative path, text)` pairs in, `Project` out. A code file the parser
/// fails on is not in `files`; it is listed in `unreadable`. Other non-code
/// paths are in neither.
pub fn project(sources: impl IntoIterator<Item = (String, String)>) -> Project {
    let mut files = BTreeMap::new();
    let mut unreadable = Vec::new();
    let mut crates = BTreeSet::new();
    let mut twins = Vec::new();
    // C/C++ files with a class head or function name the grammar misread,
    // read again once every file's macros are known: path, text, the
    // misread words, file as read.
    let mut misread = Vec::new();
    for (path, text) in sources {
        if manifest(&path) {
            let dir = path.trim_end_matches("Cargo.toml").trim_end_matches('/');
            crates.extend(crate_names(&text).into_iter().map(|n| (n, dir.to_string())));
            continue;
        }
        if twin(&path) {
            twins.push(path);
            continue;
        }
        let Some(a) = analyze(&path, &text) else {
            if handles(&path) {
                unreadable.push(path);
            }
            continue;
        };
        let file = file(&path, &text, &a);
        let heads = match file.family {
            Family::Cpp => cpp::misread_heads(&a),
            Family::Rust | Family::Python | Family::Go | Family::Shell | Family::Ecma => Vec::new(),
        };
        if heads.is_empty() {
            files.insert(path, file);
        } else {
            misread.push((path, text, heads, file));
        }
    }
    let macros: BTreeSet<String> = (files.values().chain(misread.iter().map(|m| &m.3)))
        .flat_map(|f| f.macros.iter().map(|m| m.0.clone()))
        .collect();
    for (path, text, heads, read) in misread {
        let blanked = cpp::unmacroed(&text, &heads, &macros);
        let a = blanked.and_then(|t| analyze(&path, &t));
        let f = a.map_or(read, |a| file(&path, &text, &a));
        files.insert(path, f);
    }
    pair_declarations(&mut files);
    hidden_jumps(&mut files);
    unreadable.sort();
    twins.sort();
    Project {
        files,
        unreadable,
        crates,
        twins,
    }
}

/// The file at `path` of the text `text`, analysed as `a`.
fn file(path: &str, text: &str, a: &Analyzed) -> File {
    let family = Family::of(Some(&a.file.lang));
    let python = match family {
        Family::Python => true,
        Family::Rust | Family::Cpp | Family::Go | Family::Shell | Family::Ecma => false,
    };
    let mut file = File {
        imports: if python {
            flow::module_imports(a)
        } else {
            Vec::new()
        },
        module_code: match family {
            Family::Python => flow::module_code(a),
            Family::Cpp => flow::cpp_module_code(a),
            Family::Rust | Family::Go | Family::Shell | Family::Ecma => false,
        },
        test_fns: 0,
        family,
        syntax_errors: a.sources.iter().any(|s| s.tree().root_node().has_error()),
        lines: text.lines().count().max(1),
        code: a.file.clone(),
        flows: BTreeMap::new(),
        declarations: BTreeMap::new(),
        macros: if family == Family::Cpp {
            flow::cpp_macros(a)
        } else {
            Vec::new()
        },
        local: BTreeSet::new(),
        params: BTreeMap::new(),
        namespaces: BTreeSet::new(),
        namespace_aliases: BTreeMap::new(),
        scopes: BTreeMap::new(),
        usings: BTreeSet::new(),
        file_usings: BTreeSet::new(),
        inner_usings: BTreeSet::new(),
        classes: BTreeMap::new(),
        bases: BTreeMap::new(),
        aliases: BTreeMap::new(),
        fields: BTreeMap::new(),
        variables: BTreeSet::new(),
        specialized: BTreeSet::new(),
    };
    flow::attach(&mut file, a);
    if family == Family::Cpp {
        cpp_facts(&mut file, a, path);
    }
    file
}

/// Counts in each C/C++ flow every bare call and every name written as a
/// statement, nested flows' included, of a name some file of the project
/// defines as a macro with a jump.
fn hidden_jumps(files: &mut BTreeMap<String, File>) {
    let jumps: BTreeSet<String> = (files.values().flat_map(|f| &f.macros))
        .filter(|m| m.1)
        .map(|m| m.0.clone())
        .collect();
    for f in files.values_mut().filter(|f| f.family == Family::Cpp) {
        for flow in f.flows.values_mut() {
            let mut n = 0;
            flow.each_call(&mut |_, c| {
                n += usize::from(c.site.via == CallVia::Bare && jumps.contains(&c.site.name));
            });
            n += flow.words.iter().filter(|w| jumps.contains(*w)).count();
            if n > 0 {
                *flow.not_drawn.entry(NotDrawn::HiddenJump).or_default() += n;
            }
        }
    }
}

/// `t` with its template arguments and whitespace dropped:
/// `a::Box<T>::Node` is `a::Box::Node`.
pub(crate) fn untemplated(t: &str) -> String {
    let mut depth = 0usize;
    let kept = t.chars().filter(|&c| {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ => return depth == 0 && !c.is_whitespace(),
        }
        false
    });
    kept.collect()
}

/// What a call on a value of the written type `t` lands in: whether `t`
/// is a `std::unique_ptr` or `std::shared_ptr`, and the class it names,
/// the pointee's for one, `const` and `volatile` dropped (`Book` of
/// `std::unique_ptr<Book const>`).
pub(crate) fn pointee(t: &str) -> (bool, &str) {
    let (head, arg) = t.split_once('<').unwrap_or((t, ""));
    let (smart, mut class) = match head.trim().trim_start_matches("std::") {
        "unique_ptr" | "shared_ptr" => {
            let arg = arg.rsplit_once('>').map_or(arg, |a| a.0);
            (true, arg.split(',').next().unwrap_or("").trim())
        }
        _ => (false, t.trim()),
    };
    let cv = |w: &str| matches!(w, "const" | "volatile");
    loop {
        if let Some((_, rest)) = class.split_once(' ').filter(|w| cv(w.0)) {
            class = rest.trim_start();
        } else if let Some((rest, _)) = class.rsplit_once(' ').filter(|w| cv(w.1)) {
            class = rest.trim_end();
        } else {
            return (smart, class);
        }
    }
}

/// An Objective-C, Objective-C++ or CUDA source file: no grammar reads
/// it. A CUDA header (`.cuh`) is left out: it defines what another
/// header of its stem declares no more than any header does.
fn twin(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name
        .rsplit_once('.')
        .filter(|n| !n.0.is_empty())
        .map(|n| n.1.to_ascii_lowercase());
    ext.is_some_and(|e| matches!(e.as_str(), "m" | "mm" | "cu"))
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
    let walked = walk(root, true, &|p| handles(p) || manifest(p) || twin(p));
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
    fn project_ignores_input_order_and_keeps_call_sites() {
        let p = project([
            ("src/b.rs".to_string(), "pub fn h() {}\n".to_string()),
            ("src/a.rs".to_string(), "fn f() { h(); }\n".to_string()),
        ]);
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
