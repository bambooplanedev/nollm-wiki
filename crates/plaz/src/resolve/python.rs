//! Python's own call rules: the file an import names, the class a `self`,
//! `cls` or `super()` call starts from, and dotted receivers.

use super::{Caller, Evidence};
use crate::flow::FlowScope;
use crate::model::{File, MapCall};
use crate::uses::{bases, python_import, PyRoots, Whose};
use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::{owner_key, Family, FnRef};
use wiki_code::model::{CodeItem, CodeItemKind};
use wiki_code::support::module_stem;

/// What the project's Python files say before any call is looked at. A
/// page is a position in the project's key order, as in the index.
pub(super) struct Python<'a> {
    pages: Vec<(&'a str, &'a File)>,
    pub roots: PyRoots<'a>,
    /// `pkg/mod` of `pkg/mod.py`, `pkg` of `pkg/__init__.py`, and its page.
    modules: Vec<(&'a str, usize)>,
    /// Per page and name, as a call site gives the name: the pages the
    /// page's imports say it comes from: none when the module is no file
    /// of the project. No entry when no import of the project's names it.
    named: BTreeMap<usize, BTreeMap<&'a str, BTreeSet<usize>>>,
    /// Per page: the pages its `from m import *` name.
    globs: BTreeMap<usize, BTreeSet<usize>>,
    /// Per page: the names its imports from outside the project bind.
    outside: BTreeMap<usize, BTreeSet<&'a str>>,
    /// Per class name: its definitions.
    classes: BTreeMap<&'a str, Vec<FnRef>>,
    /// Per class name: the classes naming it as a base.
    subclasses: BTreeMap<&'a str, BTreeSet<&'a str>>,
}

/// The name the base written `base` has where it is defined, by the file's
/// `import … as`.
fn unaliased<'a>(file: &'a File, base: &'a str) -> &'a str {
    let import = file
        .imports
        .iter()
        .find(|i| i.alias.as_deref() == Some(base));
    import.and_then(|i| i.name.as_deref()).unwrap_or(base)
}

impl<'a> Python<'a> {
    pub fn new(files: &'a BTreeMap<String, File>) -> Python<'a> {
        let pages: Vec<(&str, &File)> = files.iter().map(|(p, f)| (p.as_str(), f)).collect();
        let python: Vec<(usize, &str, &File)> = pages
            .iter()
            .enumerate()
            .filter(|(_, (_, f))| f.family == Family::Python)
            .map(|(page, &(path, file))| (page, path, file))
            .collect();
        let mut py = Python {
            roots: PyRoots::default(),
            modules: python
                .iter()
                .map(|&(page, p, _)| {
                    let m = p.rsplit_once('.').map_or(p, |(m, _)| m);
                    let m = m.strip_suffix("__init__").unwrap_or(m);
                    (m.trim_end_matches('/'), page)
                })
                .collect(),
            named: BTreeMap::new(),
            globs: BTreeMap::new(),
            outside: BTreeMap::new(),
            classes: BTreeMap::new(),
            subclasses: BTreeMap::new(),
            pages,
        };
        // Python 3 imports are absolute: inside a package a directory or
        // module is reached only through that package (`app.json`).
        let packages: BTreeSet<&str> = python
            .iter()
            .filter_map(|&(_, p, _)| p.strip_suffix("/__init__.py"))
            .collect();
        for &(_, path, _) in &python {
            let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
            let mut start = 0;
            for seg in dir.split('/') {
                // A root: a directory whose parent is no package.
                if !packages.contains(dir[..start].trim_end_matches('/')) {
                    py.roots.dirs.insert(seg);
                }
                start += seg.len() + 1;
            }
            if !packages.contains(dir) {
                let dirs = py.roots.stems.entry(module_stem(path)).or_default();
                dirs.insert(dir);
            }
        }
        for (page, path, file) in python {
            for imp in &file.imports {
                if python_import(imp, path, file, &py.roots) != Whose::Project {
                    py.outside.entry(page).or_default().extend(imp.binds());
                    continue;
                }
                let module = imp.module.as_str();
                let mut from = py.files(path, module);
                let key = match imp.name.as_deref() {
                    Some("*") => {
                        py.globs.entry(page).or_default().extend(from);
                        continue;
                    }
                    Some(name) => {
                        from.extend(py.files(path, &imp.path()));
                        name
                    }
                    // `import a.b` binds `a`: a call names its module in full.
                    None if imp.alias.is_none() && module.contains('.') => continue,
                    None => module.rsplit('.').next().unwrap_or(""),
                };
                // No file of the module: the name is none of the project's
                // namesakes.
                let names = py.named.entry(page).or_default();
                names.entry(key).or_default().extend(from);
            }
            let types = file.code.items.iter().enumerate();
            for (i, it) in types.filter(|(_, it)| it.kind == CodeItemKind::Type && !it.test) {
                py.classes.entry(&it.name).or_default().push((page, i));
                for b in bases(it) {
                    let b = unaliased(file, b);
                    py.subclasses.entry(b).or_default().insert(&it.name);
                }
            }
        }
        py
    }

    /// The pages of the module `module` (`a.b`, `.m`, `..`) imported in the
    /// file at `from`. A relative module is one file. An absolute one is
    /// every file its dotted path ends: which directory is the import root
    /// is not in the code.
    fn files(&self, from: &str, module: &str) -> BTreeSet<usize> {
        let rest = module.trim_start_matches('.');
        let dots = module.len() - rest.len();
        let segs = rest.split('.').filter(|s| !s.is_empty());
        let pick = |keep: &dyn Fn(&str) -> bool| {
            let found = self.modules.iter().filter(|(m, _)| keep(m));
            found.map(|&(_, page)| page).collect()
        };
        if dots == 0 {
            let want = segs.collect::<Vec<_>>().join("/");
            let tail = format!("/{want}");
            return pick(&|m| m == want || m.ends_with(&tail));
        }
        let mut dir: Vec<&str> = from.split('/').collect();
        dir.pop();
        if dots - 1 > dir.len() {
            return BTreeSet::new();
        }
        dir.truncate(dir.len() - (dots - 1));
        let want = dir.into_iter().chain(segs).collect::<Vec<_>>().join("/");
        pick(&|m| m == want)
    }

    /// `pages`, and every page a name `name` or a glob import of one of
    /// them may bring it from (`from .core import run` in `__init__.py`).
    pub fn through(&self, pages: BTreeSet<usize>, name: &str) -> BTreeSet<usize> {
        let mut todo: Vec<usize> = pages.into_iter().collect();
        let mut seen = BTreeSet::new();
        while let Some(p) = todo.pop() {
            if seen.insert(p) {
                let named = self.named.get(&p).and_then(|n| n.get(name));
                todo.extend(named.into_iter().chain(self.globs.get(&p)).flatten());
            }
        }
        seen
    }

    /// The pages `name` comes from in the file of `page`, by its imports;
    /// `None` when no import of the project's says.
    pub fn sources(&self, page: usize, name: &str) -> Option<BTreeSet<usize>> {
        let named = self.named.get(&page)?.get(name)?;
        Some(self.through(named.clone(), name))
    }

    fn item(&self, c: FnRef) -> &'a CodeItem {
        &self.pages[c.0].1.code.items[c.1]
    }

    /// The classes the file of `page` calls `written`: its own, else the
    /// ones its imports name, else every class of that name; none when an
    /// import from outside the project binds the name.
    fn classes_named(&self, page: usize, written: &str) -> Vec<FnRef> {
        let name = unaliased(self.pages[page].1, written);
        let all = self.classes.get(name).map_or(&[][..], Vec::as_slice);
        let cut = |keep: &dyn Fn(usize) -> bool| -> Vec<FnRef> {
            all.iter().copied().filter(|c| keep(c.0)).collect()
        };
        let own = cut(&|p| p == page);
        let outside = self.outside.get(&page).is_some_and(|o| o.contains(written));
        let imported = self
            .sources(page, name)
            .map(|from| cut(&|p| from.contains(&p)));
        match (own.is_empty(), imported) {
            (false, _) => own,
            (true, _) if outside => Vec::new(),
            (true, Some(i)) if !i.is_empty() => i,
            (true, _) => all.to_vec(),
        }
    }

    /// The methods named `name` the class `class` defines itself.
    fn methods(&self, class: FnRef, name: &str) -> Vec<FnRef> {
        let cls = self.item(class);
        let items = self.pages[class.0].1.code.items.iter().enumerate();
        items
            .filter(|(_, it)| {
                it.kind == CodeItemKind::Method
                    && it.name == name
                    && !it.nested
                    && it.owner.as_deref().map(owner_key) == Some(cls.name.as_str())
                    && (cls.start_line..=cls.end_line).contains(&it.start_line)
            })
            .map(|(i, _)| (class.0, i))
            .collect()
    }

    /// The methods `name` that `class` inherits: those of the first base,
    /// in the order written, that has one, itself or inherited. `true`
    /// when a base from outside the project may have it first.
    fn inherited(&self, class: FnRef, name: &str, depth: usize) -> (Vec<FnRef>, bool) {
        let mut open = false;
        for b in bases(self.item(class)) {
            let found = self.classes_named(class.0, b);
            if found.is_empty() || depth == 0 {
                open |= b != "object";
                continue;
            }
            let mut fns = Vec::new();
            for c in found {
                let own = self.methods(c, name);
                if own.is_empty() {
                    let (up, o) = self.inherited(c, name, depth - 1);
                    fns.extend(up);
                    open |= o;
                } else {
                    fns.extend(own);
                }
            }
            if !fns.is_empty() {
                return (fns, open);
            }
        }
        (Vec::new(), open)
    }
}

/// How a call names the caller's own class.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Own {
    /// `self.f()`, `cls.f()`: the object may be a subclass's.
    Object,
    /// `super().f()`.
    Super,
}

impl Caller<'_, '_> {
    /// Is `x` the class in a classmethod of the caller's own? The first
    /// parameter of a method that is no staticmethod, named `cls`.
    pub(super) fn is_cls(&self, x: &str) -> bool {
        let sig = self.item.signature.as_str();
        let params = sig.split_once(&format!("def {}(", self.item.name));
        let first = params.and_then(|(_, p)| p.split([',', ')', ':', '=']).next());
        self.family == Family::Python
            && x == "cls"
            && self.owner.is_some()
            && first.map(str::trim) == Some("cls")
            && !sig.contains("@staticmethod")
    }

    /// A call on the caller's own class `own`: its own method, else the
    /// one it inherits from the project's classes, and for an object every
    /// override in a project subclass. None of them: any method, or for
    /// `super()` under a base from outside the project, outside.
    pub(super) fn py_on_self(&self, call: &MapCall, own: &str, how: Own) -> Evidence {
        let name = call.site.name.as_str();
        let line = self.item.start_line;
        let classes = self.py.classes.get(own).into_iter().flatten();
        let class = classes.copied().find(|&c| {
            let it = self.py.item(c);
            c.0 == self.page && (it.start_line..=it.end_line).contains(&line)
        });
        let Some(class) = class else {
            return self.any_method(call);
        };
        let mine = match how {
            Own::Object => self.py.methods(class, name),
            Own::Super => Vec::new(),
        };
        let (mut fns, open) = if mine.is_empty() {
            self.py.inherited(class, name, 16)
        } else {
            (mine, false)
        };
        if how == Own::Object {
            fns.extend(self.overrides(own, call));
        }
        if fns.is_empty() && how == Own::Super && !bases(self.py.item(class)).is_empty() {
            // Project bases that end in `object`, or a base from outside.
            return Evidence::Outside;
        }
        if fns.is_empty() {
            return self.any_method(call);
        }
        Evidence::Found {
            fns: self.ids(fns),
            open,
        }
    }

    /// The methods of the call's name in every project subclass of the
    /// Python class `class`, by the bases their signatures name.
    pub(super) fn overrides(&self, class: &str, call: &MapCall) -> Vec<FnRef> {
        let mut seen = BTreeSet::new();
        let mut todo = vec![class];
        let mut out = Vec::new();
        while let Some(c) = todo.pop() {
            for &sub in self.py.subclasses.get(c).into_iter().flatten() {
                if seen.insert(sub) {
                    todo.push(sub);
                    let own = self.scope.qualified(&call.site, sub).into_iter();
                    out.extend(own.filter(|&f| self.index.item(f).kind == CodeItemKind::Method));
                }
            }
        }
        out
    }

    /// Does the Python module bind `x` to a value or function of its
    /// own (`config = load()`), not to a module or class?
    pub(super) fn py_value(&self, x: &str) -> bool {
        !self.names.imported.contains(x)
            && self
                .file
                .code
                .items
                .iter()
                .any(|it| it.name == x && it.owner.is_none() && it.kind != CodeItemKind::Type)
    }

    /// `fns`, the `attr`s of `name`, cut to the files the caller's imports
    /// say `name` comes from and those bring `attr` from; all of them when
    /// no import says.
    pub(super) fn imported_from(&self, name: &str, attr: &str, fns: Vec<FnRef>) -> Vec<FnRef> {
        match self.py.sources(self.page, name) {
            Some(from) => {
                let from = self.py.through(from, attr);
                fns.into_iter().filter(|c| from.contains(&c.0)).collect()
            }
            None => fns,
        }
    }

    /// Does a body around the call bind the name the call is written
    /// with, where the call site names what a file-level `import … as`
    /// says that name stands for?
    pub(super) fn rebinds_alias(&self, chain: &[FlowScope], call: &MapCall) -> bool {
        let written = call.text.split('(').next().unwrap_or("").trim();
        let bound = |f: &FlowScope| f.locals.contains(written) || f.rebound.contains(written);
        self.family == Family::Python && written != call.site.name && chain.iter().any(bound)
    }

    /// Is the receiver written as an alias a project import binds, where
    /// the call site names what it stands for (`au.u()` after `from app
    /// import utils as au`, beside an outside `import utils`)?
    pub(super) fn project_alias(&self, call: &MapCall, x: &str) -> bool {
        let written = call.text.split(['.', '(']).next().unwrap_or("").trim();
        written != x
            && self.names.aliases.contains(written)
            && !self.names.external.contains_key(written)
    }

    /// A Python call whose receiver is no plain name. `super().f()`: the
    /// inherited `f`. `a.b.f()` under a module the file imports from the
    /// project: `f` of the module `a.b`, or of a class `b` of `a`; from
    /// outside, outside. A receiver built on a name the body rebinds
    /// (`Local().f()`) is not visible; anything else (`self.x.f()`,
    /// `a().f()`) is any method.
    pub(super) fn dotted(&self, chain: &[FlowScope], call: &MapCall) -> Evidence {
        let name = call.site.name.as_str();
        let word = |s: &&str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
        let callee = call.text.split('(').next().unwrap_or("");
        let segs: Vec<&str> = callee.split('.').collect();
        let root = segs[0];
        let bound = |by: &dyn Fn(&FlowScope) -> bool| chain.iter().any(by);
        if call.text.starts_with(&format!("super().{name}(")) {
            let own = self.owner.map_or("", owner_key);
            return self.py_on_self(call, own, Own::Super);
        }
        if segs.len() < 3 || !segs.iter().all(word) || segs.last() != Some(&name) {
            return if word(&root) && bound(&|f| f.rebound.contains(root)) {
                Evidence::Hidden
            } else {
                self.any_method(call)
            };
        }
        let q = segs[segs.len() - 2];
        if bound(&|f| f.locals.contains(root) || f.rebound.contains(root)) {
            self.any_method(call)
        } else if let Some(&std) = self.names.external.get(root) {
            self.external(std, self.scope.qualified(&call.site, q))
        } else if self.names.imported.contains(root) {
            // An alias stands for its module: `am` of `import app.models as am`.
            let mut alias = self.file.imports.iter();
            let alias = alias.find(|i| i.alias.as_deref() == Some(root));
            let head = alias.map_or(root.to_string(), crate::model::Import::path);
            let module = |n: usize| {
                let path = std::iter::once(head.as_str()).chain(segs[1..n].iter().copied());
                let path = path.collect::<Vec<_>>().join(".");
                self.py.files(self.index.key(self.page), &path)
            };
            let mut from = module(segs.len() - 1);
            from.extend(self.py.through(module(segs.len() - 2), q));
            let from = self.py.through(from, name);
            let fns = self.scope.qualified(&call.site, q).into_iter();
            // No file of that path: none of the project's namesakes.
            let fns = fns.filter(|c| from.contains(&c.0));
            self.found_or_hidden(fns.collect(), !self.sure(root))
        } else {
            self.any_method(call)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_is_one_file_when_relative_and_every_path_ending_when_absolute() {
        let paths = [
            "__init__.py",
            "a.py",
            "pkg/__init__.py",
            "pkg/a.py",
            "pkg/mod.py",
            "pkg/sub/deep.py",
            "top/mod.py",
            "src/json.rs",
        ];
        let p =
            crate::source::project(paths.map(|p| (p.to_string(), "def f():\n    pass\n".into())));
        let py = Python::new(&p.files);
        let files = |from: &str, module: &str| -> Vec<&str> {
            let found = py.files(from, module);
            found.into_iter().map(|page| py.pages[page].0).collect()
        };
        for (from, module, want) in [
            ("pkg/a.py", ".mod", vec!["pkg/mod.py"]),
            ("pkg/a.py", ".", vec!["pkg/__init__.py"]),
            ("pkg/a.py", ".sub.deep", vec!["pkg/sub/deep.py"]),
            ("pkg/sub/deep.py", "..mod", vec!["pkg/mod.py"]),
            ("pkg/sub/deep.py", "...a", vec!["a.py"]),
            ("pkg/a.py", "..", vec!["__init__.py"]),
            ("pkg/a.py", "....mod", vec![]),
            ("a.py", "pkg.mod", vec!["pkg/mod.py"]),
            ("a.py", "pkg", vec!["pkg/__init__.py"]),
            ("a.py", "mod", vec!["pkg/mod.py", "top/mod.py"]),
            ("a.py", "sub.deep", vec!["pkg/sub/deep.py"]),
            ("a.py", "od", vec![]),
            ("a.py", "json", vec![]),
        ] {
            assert_eq!(files(from, module), want, "{module} in {from}");
        }
    }
}
