//! The source stage: files in, an analysed `Project` out.

mod cpp;

use crate::index::Family;
use crate::links::flow;
use crate::links::model::{CppFacts, File, NotDrawn, Project};
use crate::model::CallVia;
use crate::tree_sitter::Node;
use crate::walk::walk;
use crate::{analyze, handles, is_cpp_fragment, pair_declarations, Analyzed};
use cpp::cpp_facts;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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
    // Per Rust file, its top-level `mod n;` declarations; the directories
    // holding a `Cargo.toml`.
    let (mut mods, mut packages) = (BTreeMap::new(), BTreeSet::new());
    for (path, text) in sources {
        if manifest(&path) {
            let dir = path.trim_end_matches("Cargo.toml").trim_end_matches('/');
            crates.extend(crate_names(&text).into_iter().map(|n| (n, dir.to_string())));
            packages.insert(dir.to_string());
            continue;
        }
        if twin(&path) {
            twins.push(path);
            continue;
        }
        let Some(a) = analyze(&path, &text) else {
            if handles(&path) || is_cpp_fragment(&path) {
                unreadable.push(path);
            }
            continue;
        };
        let file = file(&path, &text, &a);
        if file.family == Family::Rust {
            mods.insert(path.clone(), file_mods(&a));
        }
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
        .flat_map(|f| f.cpp.macros.iter().map(|m| m.0.clone()))
        .collect();
    for (path, text, heads, read) in misread {
        let blanked = cpp::unmacroed(&text, &heads, &macros);
        let a = blanked.and_then(|t| analyze(&path, &t));
        let f = a.map_or(read, |a| file(&path, &text, &a));
        files.insert(path, f);
    }
    drop_pieces(&mut files);
    test_modules(&mut files, &mods, &packages);
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

/// The path an `#include` written `import` (`CodeFile::imports`) may name
/// in the project: a quoted one, or an angle one naming a directory
/// (`<a/b.h>` may be a project header; `<b.h>` is a system one).
pub(crate) fn project_include(import: &str) -> Option<&str> {
    match import.strip_prefix('<') {
        Some(a) => a.strip_suffix('>').filter(|a| a.contains('/')),
        None => Some(import),
    }
}

/// Which of `named`, the files whose name is `inc`'s last segment, each
/// with its path, `#include "inc"` in the file at `from` names: the file
/// at that path relative to `from`'s directory, else every file whose
/// path ends in it (`..` segments dropped).
pub(crate) fn included<T: Copy>(named: &[(T, &str)], from: &str, inc: &str) -> Vec<T> {
    let mut dir: Vec<&str> = from.split('/').collect();
    dir.pop();
    let mut inside = true;
    for seg in inc.split('/') {
        match seg {
            "" | "." => {}
            ".." => inside &= dir.pop().is_some(),
            s => dir.push(s),
        }
    }
    let want = dir.join("/");
    if let Some(p) = named.iter().find(|p| inside && p.1 == want) {
        return vec![p.0];
    }
    let segs = inc.split('/').skip_while(|s| matches!(*s, "" | "." | ".."));
    let tail = segs.collect::<Vec<_>>().join("/");
    let ends = |p: &str| p == tail || p.ends_with(&format!("/{tail}"));
    let all = named.iter().filter(|p| ends(p.1));
    all.map(|p| p.0).collect()
}

/// Drops the files made to be included (`.inl`, `.ipp`, `.tcc`) that do
/// not stand on their own. Such a file is read only when an include of a
/// file that is read names it, it alone, and no include does inside a
/// namespace, class or function body, or from a file that is such a
/// piece: it would be a piece of that body, read without it.
fn drop_pieces(files: &mut BTreeMap<String, File>) {
    let mut by_name: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for path in files.keys() {
        let name = path.rsplit('/').next().unwrap_or(path);
        by_name.entry(name).or_default().push((path, path));
    }
    // Per include: the file writing it, the files it names, is it in a body.
    let mut includes = Vec::new();
    for (from, f) in files.iter() {
        let nested = &f.cpp.nested_includes;
        let plain = f.code.imports.iter().filter_map(|i| project_include(i));
        let plain = plain.filter(|i| !nested.contains(*i)).map(|i| (i, false));
        for (inc, nested) in plain.chain(nested.iter().map(|i| (i.as_str(), true))) {
            let named = by_name.get(inc.rsplit('/').next().unwrap_or(inc));
            let named = included(named.map_or(&[][..], Vec::as_slice), from, inc);
            includes.push((from.as_str(), named, nested));
        }
    }
    // A piece of a body, or one of several an include may name; and so
    // is whatever such a piece includes.
    let unsure = |i: &&(&str, Vec<&str>, bool)| i.2 || i.1.len() > 1;
    let mut bad: BTreeSet<&str> = (includes.iter().filter(unsure))
        .flat_map(|i| i.1.iter().copied().filter(|p| is_cpp_fragment(p)))
        .collect();
    // Read: what an include of a file that is read names, from the files
    // that are no such piece on.
    let mut read: BTreeSet<&str> = (files.keys().map(String::as_str))
        .filter(|p| !is_cpp_fragment(p))
        .collect();
    loop {
        let before = (bad.len(), read.len());
        for (from, named, _) in &includes {
            if bad.contains(from) {
                bad.extend(named.iter().filter(|p| is_cpp_fragment(p)));
            } else if read.contains(from) {
                read.extend(named.iter().filter(|p| !bad.contains(*p)));
            }
        }
        if before == (bad.len(), read.len()) {
            break;
        }
    }
    let read: BTreeSet<String> = (read.difference(&bad)).map(|p| (*p).to_string()).collect();
    files.retain(|path, _| !is_cpp_fragment(path) || read.contains(path));
}

/// The body-less `mod n;` declarations at the top level of a Rust file,
/// each with whether it is written `#[cfg(test)]` exactly; one with a
/// `path` attribute names a file elsewhere and is left out.
fn file_mods(a: &Analyzed) -> Vec<(String, bool)> {
    let Some(s) = a.sources.last() else {
        return Vec::new();
    };
    let root = s.tree().root_node();
    let (mut mods, mut attrs) = (Vec::new(), Vec::new());
    for n in root.named_children(&mut root.walk()) {
        let text = |n: Node| s.text().get(n.byte_range()).unwrap_or("");
        if n.kind() == "attribute_item" {
            attrs.push(text(n).replace(char::is_whitespace, ""));
            continue;
        }
        let name = n.child_by_field_name("name").map(text);
        if let ("mod_item", Some(name), None) = (n.kind(), name, n.child_by_field_name("body")) {
            if !attrs.iter().any(|a| a.contains("path=")) {
                mods.push((name.to_string(), attrs.iter().any(|a| a == "#[cfg(test)]")));
            }
        }
        attrs.clear();
    }
    mods
}

/// Makes test code of each Rust file a top-level `#[cfg(test)] mod n;`
/// declares, unless its file also declares `n` without it, and of each
/// module such a file declares: their items are tests, their flows
/// counted in `test_fns` instead. `n.rs` or `n/mod.rs` is beside a `mod.rs`
/// or a crate root (`lib.rs`, `main.rs`, a file in `bin/`, `examples/` or
/// `benches/`, a `build.rs` beside a `Cargo.toml`; a declared module is
/// none), in `x/` for `x.rs`.
fn test_modules(
    files: &mut BTreeMap<String, File>,
    mods: &BTreeMap<String, Vec<(String, bool)>>,
    packages: &BTreeSet<String>,
) {
    let children = |path: &str, name: &str, root: bool| {
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
        let stem = file.strip_suffix(".rs").unwrap_or(file);
        let up = dir.rsplit('/').next().unwrap_or("");
        let beside = stem == "mod"
            || root
                && (matches!(stem, "lib" | "main")
                    || matches!(up, "bin" | "examples" | "benches")
                    || (stem == "build" && packages.contains(dir)));
        let base = match (beside, dir) {
            (true, d) => d.to_string(),
            (false, "") => stem.to_string(),
            (false, d) => format!("{d}/{stem}"),
        };
        let base = if base.is_empty() { base } else { base + "/" };
        [format!("{base}{name}.rs"), format!("{base}{name}/mod.rs")]
    };
    let mut todo = Vec::new();
    for (path, decls) in mods {
        let live = |n: &String| decls.iter().any(|d| !d.1 && d.0 == *n);
        for (name, _) in decls.iter().filter(|d| d.1 && !live(&d.0)) {
            todo.extend(children(path, name, true));
        }
    }
    let mut done = BTreeSet::new();
    while let Some(path) = todo.pop() {
        let Some(f) = files.get_mut(&path).filter(|_| done.insert(path.clone())) else {
            continue;
        };
        f.test_fns += f.flows.len();
        f.flows.clear();
        f.code.items.iter_mut().for_each(|it| it.test = true);
        for (name, _) in mods.get(&path).into_iter().flatten() {
            todo.extend(children(&path, name, false));
        }
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
        cpp: CppFacts {
            macros: if family == Family::Cpp {
                flow::cpp_macros(a)
            } else {
                Vec::new()
            },
            ..CppFacts::default()
        },
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
    let jumps: BTreeSet<String> = (files.values().flat_map(|f| &f.cpp.macros))
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
/// is a `std::unique_ptr`, `std::shared_ptr` or `std::optional`, and the
/// class it names, the pointee's for one, `const` and `volatile` dropped
/// (`Book` of `std::unique_ptr<Book const>`).
pub(crate) fn pointee(t: &str) -> (bool, &str) {
    let (head, arg) = t.split_once('<').unwrap_or((t, ""));
    let (smart, mut class) = match head.trim().trim_start_matches("std::") {
        "unique_ptr" | "shared_ptr" | "optional" => {
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

pub fn load(root: &Path) -> Result<Project, std::io::Error> {
    if !root.is_dir() {
        return Err(std::io::ErrorKind::NotADirectory.into());
    }
    let read = |p: &str| handles(p) || is_cpp_fragment(p) || manifest(p) || twin(p);
    let walked = walk(root, true, &read);
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
            Err(e) if e.kind() == std::io::ErrorKind::NotADirectory
        ));
    }
}
