//! What a file's imports (Rust's `use` lines, Python's `import`
//! statements) and own items say about a name: the contradictions the
//! resolve stage gives up certainty for.

use crate::model::{File, Import};
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::imports::Imports;
use wiki_code::index::{Family, Index};
use wiki_code::model::{CodeItem, CodeItemKind};

/// What a file's imports bring into scope.
pub(crate) struct InScope {
    /// The names imports bring in from outside the project, each with
    /// "surely outside": from `std`, `core` or `alloc`, or a Python import
    /// no module of the project is named like; another crate may be the
    /// project's own library, as may a Python [`Whose::Unsure`]. A Rust
    /// import is the project's when its root is `crate`, `self` or `super`,
    /// a project module or type, one of the project's own crates, or a
    /// name the file defines (`mod app; use app::run;`); a Python one as
    /// [`python_import`] says.
    pub external: BTreeMap<String, bool>,
    /// The names any import names.
    pub imported: BTreeSet<String>,
    /// The names a `use … as` binds: not what the project calls so.
    pub aliases: BTreeSet<String>,
    /// The original names of outside `use … as` aliases that a project
    /// import names too: a call of the name may be either.
    pub contested: BTreeSet<String>,
    /// An import from outside the project ends in `*`: a name the file
    /// neither defines nor names may be that crate's.
    pub glob: bool,
    /// The names imports bring in from the project's own crates, each with
    /// the crates they come from (`use beta::resolve;`: `resolve`, `beta`).
    pub from_crate: BTreeMap<String, BTreeSet<String>>,
}

impl InScope {
    /// Does an import of the project bring in `name` from no own crate
    /// (`use crate::utils;`)?
    pub fn local(&self, name: &str) -> bool {
        self.imported.contains(name)
            && !self.external.contains_key(name)
            && !self.from_crate.contains_key(name)
    }
}

/// Does the file define `name` itself, as an item that is no method?
pub(crate) fn defines(file: &File, name: &str) -> bool {
    let mut items = file.code.items.iter();
    items.any(|it| it.name == name && it.owner.is_none() && !it.test)
}

/// Is `root` one of Rust's standard crates here? Not when the file has its
/// own `mod core;`; never in Python, whose imports say what is outside.
pub(crate) fn standard(file: &File, root: &str) -> bool {
    match file.family {
        Family::Python => false,
        Family::Rust | Family::Cpp | Family::Go | Family::Shell | Family::Ecma => {
            matches!(root, "std" | "core" | "alloc") && !defines(file, root)
        }
    }
}

/// What one import says: the names it brings in, each `(original, alias)`
/// rename, whether it is the project's, surely outside, a glob.
struct Fact {
    names: BTreeSet<String>,
    renames: Vec<(String, String)>,
    project: bool,
    std: bool,
    glob: bool,
    /// The own crate the import's path starts at.
    krate: Option<String>,
}

/// Is `root` one of the project's own crates, named in a Rust file?
pub(crate) fn own_crate(file: &File, root: &str, crates: &BTreeSet<(String, String)>) -> bool {
    file.family == Family::Rust && crates.iter().any(|(name, _)| name == root)
}

fn rust_fact(
    path: &str,
    imp: &str,
    file: &File,
    imports: &Imports,
    index: &Index<File>,
    crates: &BTreeSet<(String, String)>,
) -> Fact {
    let root = imp.split("::").next().unwrap_or("").trim();
    Fact {
        names: import_names(imp),
        // `use a::f as g`: a call of `g` reaches here as a call of `f`.
        renames: imp
            .split([',', '{', '}'])
            .filter_map(|p| p.rsplit_once(" as "))
            .map(|(path, alias)| {
                let original = path.rsplit("::").next().unwrap_or("").trim();
                (original.to_string(), alias.trim().to_string())
            })
            .collect(),
        project: matches!(root, "crate" | "self" | "super")
            || index.is_module(root)
            || index.is_type(root)
            || defines(file, root)
            || own_crate(file, root, crates)
            || !imports.resolve(path, imp).is_empty(),
        std: standard(file, root),
        glob: imp.contains('*'),
        krate: own_crate(file, root, crates).then(|| root.to_string()),
    }
}

/// What the root of an absolute Python import can name in the project:
/// import resolution alone would take `import json` for a `json.rs`.
#[derive(Default)]
pub(crate) struct PyRoots<'a> {
    /// The names of the directories of the Python files whose parent is
    /// no package (no `__init__.py`): the roots of the project's imports.
    pub dirs: BTreeSet<&'a str>,
    /// The module stems of the Python files in no package, each with its
    /// directories: a module in a package is imported through it.
    pub stems: BTreeMap<&'a str, BTreeSet<&'a str>>,
}

/// Whose module a Python import names.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Whose {
    Project,
    /// A module of the project has the name, in a directory nothing puts
    /// on the import path of the importing file.
    Unsure,
    Outside,
}

/// Whose is the import in the file at `path`? By its root alone: the
/// project's when it is relative, or its root is a package of the project,
/// a name the file defines, or a module in no package, at the top of the
/// tree or in a directory the file is under. `requests.utils` beside a
/// project `utils.py` is outside, as is `import json` beside `app/json.py`
/// in a package `app`.
pub(crate) fn python_import(imp: &Import, path: &str, file: &File, roots: &PyRoots) -> Whose {
    let root = imp.module.split('.').next().unwrap_or("");
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    let above = |d: &&str| d.is_empty() || dir == *d || dir.starts_with(&format!("{d}/"));
    if imp.module.starts_with('.') || roots.dirs.contains(root) || defines(file, root) {
        return Whose::Project;
    }
    match roots.stems.get(root) {
        Some(dirs) if dirs.iter().any(above) => Whose::Project,
        Some(_) => Whose::Unsure,
        None => Whose::Outside,
    }
}

/// `import a.b as c` reaches a call as `b`, `from m import f as g` as `f`.
fn python_fact(imp: &Import, path: &str, file: &File, roots: &PyRoots) -> Fact {
    let whose = python_import(imp, path, file, roots);
    let original = imp
        .name
        .clone()
        .unwrap_or_else(|| imp.module.rsplit('.').next().unwrap_or("").to_string());
    Fact {
        names: imp.binds().map(String::from).into_iter().collect(),
        renames: imp
            .alias
            .iter()
            .map(|a| (original.clone(), a.clone()))
            .collect(),
        project: whose == Whose::Project,
        std: whose == Whose::Outside,
        glob: imp.name.as_deref() == Some("*"),
        krate: None,
    }
}

/// `py_roots`: see [`python_import`]; `crates`: [`Project::crates`].
///
/// [`Project::crates`]: crate::model::Project::crates
pub(crate) fn in_scope(
    path: &str,
    file: &File,
    imports: &Imports,
    index: &Index<File>,
    py_roots: &PyRoots,
    crates: &BTreeSet<(String, String)>,
) -> InScope {
    let mut s = InScope {
        external: BTreeMap::new(),
        imported: BTreeSet::new(),
        aliases: BTreeSet::new(),
        contested: BTreeSet::new(),
        glob: false,
        from_crate: BTreeMap::new(),
    };
    // The original names of outside aliases, and the names project imports bring in.
    let (mut originals, mut project_names) = (BTreeMap::new(), BTreeSet::new());
    let facts: Vec<Fact> = match file.family {
        Family::Python => file
            .imports
            .iter()
            .map(|i| python_fact(i, path, file, py_roots))
            .collect(),
        Family::Rust | Family::Cpp | Family::Go | Family::Shell | Family::Ecma => file
            .code
            .imports
            .iter()
            .map(|i| rust_fact(path, i, file, imports, index, crates))
            .collect(),
    };
    for f in facts {
        let external = f.std || !f.project;
        if let Some(k) = &f.krate {
            let originals = f.renames.iter().map(|(o, _)| o);
            for n in f.names.iter().chain(originals) {
                s.from_crate.entry(n.clone()).or_default().insert(k.clone());
            }
        }
        if external {
            s.external
                .extend(f.names.iter().map(|n| (n.clone(), f.std)));
            s.glob |= f.glob;
        } else {
            project_names.extend(f.names.iter().cloned());
        }
        for (original, alias) in f.renames {
            s.aliases.insert(alias);
            if external {
                originals.insert(original.clone(), f.std);
            }
            s.imported.insert(original);
        }
        s.imported.extend(f.names);
    }
    for (original, std) in originals {
        if project_names.contains(&original) {
            s.contested.insert(original);
        } else {
            s.external.insert(original, std);
        }
    }
    s
}

/// The names one import brings in: the last segment (or alias) of the
/// import, or of each member of its brace lists, a `self` member naming the
/// path before its `{`. A glob names nothing.
fn import_names(imp: &str) -> BTreeSet<String> {
    let last = |s: &str| {
        s.trim()
            .trim_end_matches("::")
            .rsplit("::")
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let mut out = BTreeSet::new();
    // The path before each open `{`, innermost last.
    let mut parents: Vec<String> = Vec::new();
    let mut member = |piece: &str, parents: &[String]| {
        let name = match piece.rsplit_once(" as ") {
            Some((_, alias)) => alias.trim().to_string(),
            None if piece.trim() == "self" => parents.last().cloned().unwrap_or_default(),
            None => last(piece),
        };
        if !matches!(name.as_str(), "" | "self" | "*") {
            out.insert(name);
        }
    };
    let mut start = 0;
    for (i, c) in imp.char_indices() {
        if matches!(c, ',' | '{' | '}') {
            let piece = &imp[start..i];
            if c == '{' {
                parents.push(last(piece));
            } else {
                member(piece, &parents);
            }
            if c == '}' {
                parents.pop();
            }
            start = i + 1;
        }
    }
    member(&imp[start..], &parents);
    out
}

/// The last segments of the bases a Python class's signature names: `B`
/// and `Mixin` of `class A(m.B, Mixin, metaclass=M)`.
pub(crate) fn bases(t: &CodeItem) -> Vec<&str> {
    let Some((_, rest)) = t.signature.split_once(&format!("class {}", t.name)) else {
        return Vec::new();
    };
    let Some(list) = rest.trim_start().strip_prefix('(') else {
        return Vec::new();
    };
    list.rsplit_once(')')
        .map_or(list, |(l, _)| l)
        .split(',')
        .filter(|b| !b.contains('='))
        .map(|b| {
            b.split('[')
                .next()
                .unwrap_or("")
                .rsplit('.')
                .next()
                .unwrap_or("")
                .trim()
        })
        .filter(|b| !b.is_empty())
        .collect()
}

/// Is `t` a trait? Its signature says `trait` before its name.
pub(crate) fn is_trait(t: &CodeItem) -> bool {
    t.kind == CodeItemKind::Type
        && t.signature
            .split_whitespace()
            .take_while(|w| w.split(['<', ':']).next() != Some(t.name.as_str()))
            .any(|w| w == "trait")
}

/// Is item `item` a method declared in a trait's body? Its owner is a
/// trait of the same file whose lines hold the method's.
pub(crate) fn in_trait(file: &File, item: usize) -> bool {
    let it = &file.code.items[item];
    let Some(owner) = it.owner.as_deref() else {
        return false;
    };
    file.code.items.iter().any(|t| {
        t.name == owner && is_trait(t) && (t.start_line..=t.end_line).contains(&it.start_line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_names_are_last_segments_aliases_and_brace_members_of_unresolved_imports() {
        let p = crate::source::project([(
            "src/a.rs".to_string(),
            "use std::{fmt, io::{self, Read}};\nuse foo::bar as baz;\nuse crate::gone::X;\nuse other::*;\nfn f() {}\n"
                .to_string(),
        )]);
        let (imports, index) = (Imports::new(&p.files), Index::new(&p.files));
        let none = PyRoots::default();
        let a = &p.files["src/a.rs"];
        let scope = in_scope("src/a.rs", a, &imports, &index, &none, &p.crates);
        // `bar` too: a call of the alias `baz` is recorded as a call of `bar`.
        assert_eq!(
            scope
                .external
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["Read", "bar", "baz", "fmt", "io"]
        );
        assert!(scope.glob && scope.aliases.contains("baz"));
    }
}
