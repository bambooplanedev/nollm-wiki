//! The source stage: files in, an analysed `Project` out.

use crate::flow;
use crate::model::{File, NotDrawn, Project};
use crate::Error;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use wiki_code::index::Family;
use wiki_code::model::{CallVia, CodeItemKind};
use wiki_code::tree_sitter::Node;
use wiki_code::walk::walk;
use wiki_code::{analyze, handles, pair_declarations, Analyzed};

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
            module_code: match family {
                Family::Python => flow::module_code(&a),
                Family::Cpp => flow::cpp_module_code(&a),
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
                flow::cpp_macros(&a)
            } else {
                Vec::new()
            },
            local: BTreeSet::new(),
            params: BTreeMap::new(),
            namespaces: BTreeSet::new(),
            scopes: BTreeMap::new(),
            usings: BTreeSet::new(),
            file_usings: BTreeSet::new(),
            inner_usings: BTreeSet::new(),
            classes: BTreeMap::new(),
            bases: BTreeMap::new(),
            aliases: BTreeSet::new(),
            fields: BTreeMap::new(),
        };
        flow::attach(&mut file, &a);
        if family == Family::Cpp {
            cpp_facts(&mut file, &a, &path);
        }
        files.insert(path, file);
    }
    pair_declarations(&mut files);
    hidden_jumps(&mut files);
    unreadable.sort();
    Project {
        files,
        unreadable,
        crates,
    }
}

/// Counts in each C/C++ flow every bare call, nested flows' included, of
/// a name some file of the project defines as a macro with a jump.
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
            if n > 0 {
                *flow.not_drawn.entry(NotDrawn::HiddenJump).or_default() += n;
            }
        }
    }
}

/// What a C/C++ file's trees show that its items do not: which functions
/// are file-local, each function's parameter types and namespace, each
/// method's class, the file's classes and their bases, namespace names
/// and `using`s.
fn cpp_facts(file: &mut File, a: &Analyzed, path: &str) {
    let anon = format!("(anonymous@{path})");
    let items = file.code.items.iter().zip(&a.defs).enumerate();
    for (i, (it, def)) in items.filter(|(_, (it, _))| !it.nested) {
        let fns = matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method);
        let Some((def, src)) = def.filter(|_| fns).map(|d| (d, &a.sources[d.source])) else {
            continue;
        };
        let Some(node) = src.node(&def) else {
            continue;
        };
        let text = |n: Node| &src.text()[n.byte_range()];
        let decl = match node.kind() {
            "template_declaration" => node.named_children(&mut node.walk()).last(),
            _ => Some(node),
        };
        let kids = decl
            .into_iter()
            .flat_map(|d| d.children(&mut d.walk()).collect::<Vec<_>>());
        let is_static = kids
            .filter(|c| c.kind() == "storage_class_specifier")
            .any(|c| text(c) == "static");
        let anonymous = std::iter::successors(node.parent(), Node::parent)
            .any(|n| n.kind() == "namespace_definition" && n.child_by_field_name("name").is_none());
        if it.kind == CodeItemKind::Fn && (is_static || anonymous) {
            file.local.insert(i);
        }
        let f = declarator(node, &it.name, src.text());
        if let Some(p) = f.and_then(|f| params(f, src.text())) {
            file.params.insert(i, p);
        }
        // The enclosing named namespaces, then the qualifier of `a::B::f`.
        let mut path = enclosing(node, src.text(), None);
        let declared = f.and_then(|f| f.child_by_field_name("declarator"));
        let qualifier = declared
            .and_then(|d| text(d).rsplit_once("::"))
            .map(|q| q.0);
        let segs = || qualifier.into_iter().flat_map(|q| q.split("::"));
        path.extend(segs().map(|s| s.trim().to_string()));
        path.retain(|s| !s.is_empty());
        if !path.is_empty() {
            file.scopes.insert(i, path.join("::"));
        }
        let in_class = std::iter::successors(node.parent(), Node::parent)
            .any(|n| n.kind() == "field_declaration_list");
        let names_owner =
            |o: &String| declared.is_some_and(|d| text(d).contains(&format!("{o}::")));
        let mut class = enclosing(node, src.text(), Some(&anon));
        if !in_class {
            class.extend(segs().map(|s| s.trim().to_string()));
        }
        let class =
            (in_class || it.owner.as_ref().is_some_and(names_owner)).then(|| class.join("::"));
        if let Some(c) = class.filter(|_| it.kind == CodeItemKind::Method) {
            file.classes.insert(i, untemplated(&c));
        }
    }
    for s in &a.sources {
        let mut stack = vec![s.tree().root_node()];
        while let Some(n) = stack.pop() {
            if let Some(name) = matches!(
                n.kind(),
                "namespace_definition" | "namespace_alias_definition"
            )
            .then(|| n.child_by_field_name("name"))
            .flatten()
            {
                let segs = segments(&s.text()[name.byte_range()]);
                file.namespaces.extend(segs.map(|g| g.0.to_string()));
            }
            class_head(file, n, s.text(), &anon);
            alias(file, n, s.text(), &anon);
            if n.kind() == "using_declaration" {
                using(file, n, s.text());
            }
            stack.extend(n.named_children(&mut n.walk()));
        }
    }
}

/// Records the class head `n` (a class, struct or union with a body) in
/// `File::bases` and its data members in `File::fields`; `anonymous`
/// names the file's anonymous namespaces.
fn class_head(file: &mut File, n: Node, src: &str, anonymous: &str) {
    let class = matches!(
        n.kind(),
        "class_specifier" | "struct_specifier" | "union_specifier"
    );
    let body = n.child_by_field_name("body");
    let name = body.and_then(|_| n.child_by_field_name("name"));
    let (Some(name), Some(body)) = (name.filter(|_| class), body) else {
        return;
    };
    let text = |n: Node| &src[n.byte_range()];
    let params = template_params(n, src);
    let clause = n
        .children(&mut n.walk())
        .find(|c| c.kind() == "base_class_clause");
    let named = clause.map(|c| c.named_children(&mut c.walk()).collect::<Vec<_>>());
    let bases = named
        .into_iter()
        .flatten()
        .filter(|b| b.kind() != "access_specifier");
    let dependent = |b: &str| {
        let args = b.split_once('<').map_or("", |a| a.1);
        args.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|w| params.contains(w))
    };
    let mut path = enclosing(n, src, Some(anonymous));
    path.push(text(name).to_string());
    let class = untemplated(&path.join("::"));
    let bases = bases.map(|b| (untemplated(text(b)), dependent(text(b))));
    file.bases.entry(class.clone()).or_default().extend(bases);
    // A member the file's two trees type differently has no type.
    let fields = file.fields.entry(class).or_default();
    for (name, ty) in flow::cpp_fields(body, src, &params) {
        let kept = fields.entry(name).or_insert_with(|| ty.clone());
        if *kept != ty {
            *kept = None;
        }
    }
}

/// Records what the `using` declaration or directive `n` names in
/// `File::usings`, and by where it stands in `file_usings` or
/// `inner_usings`.
fn using(file: &mut File, n: Node, src: &str) {
    let named = n.named_children(&mut n.walk()).last();
    let Some(named) = named.map(|u| src[u.byte_range()].split_whitespace().collect::<String>())
    else {
        return;
    };
    // `::Cfg` is `Cfg` of the global namespace.
    let named = named.trim_start_matches("::").to_string();
    let mut around = std::iter::successors(n.parent(), Node::parent);
    let scope = around.find(|p| {
        matches!(p.kind(), "compound_statement" | "field_declaration_list")
            || (p.kind() == "namespace_definition" && p.child_by_field_name("name").is_some())
    });
    match scope.map(|p| p.kind()) {
        None => file.file_usings.insert(named.clone()),
        Some("field_declaration_list") => false,
        Some(_) => file.inner_usings.insert(named.clone()),
    };
    file.usings.insert(named);
}

/// Records the type alias `n` declares, if it is one outside any body,
/// in `File::aliases`.
fn alias(file: &mut File, n: Node, src: &str, anonymous: &str) {
    let text = |n: Node| &src[n.byte_range()];
    let names: Vec<&str> = match n.kind() {
        "alias_declaration" => n
            .child_by_field_name("name")
            .map(text)
            .into_iter()
            .collect(),
        "type_definition" => {
            let specifier = |t: &Node| {
                matches!(
                    t.kind(),
                    "struct_specifier" | "class_specifier" | "union_specifier" | "enum_specifier"
                )
            };
            let class = (n.child_by_field_name("type").filter(specifier))
                .and_then(|t| t.child_by_field_name("name"));
            let mut cursor = n.walk();
            let declared = n.children_by_field_name("declarator", &mut cursor);
            let found = declared.filter_map(|mut d| {
                while d.kind() != "type_identifier" {
                    d = d.child_by_field_name("declarator")?;
                }
                Some(text(d))
            });
            // `typedef struct X X` names the class itself.
            let found: Vec<&str> = found.collect();
            found
                .into_iter()
                .filter(|&d| class.map(text) != Some(d))
                .collect()
        }
        _ => return,
    };
    let body =
        std::iter::successors(n.parent(), Node::parent).any(|p| p.kind() == "compound_statement");
    for name in names.into_iter().filter(|_| !body) {
        let mut path = enclosing(n, src, Some(anonymous));
        path.push(name.to_string());
        file.aliases.insert(untemplated(&path.join("::")));
    }
}

/// The names of the template parameters of the templates around `n`.
fn template_params<'s>(n: Node, src: &'s str) -> BTreeSet<&'s str> {
    let mut out = BTreeSet::new();
    for t in std::iter::successors(n.parent(), Node::parent) {
        let Some(list) = t
            .child_by_field_name("parameters")
            .filter(|_| t.kind() == "template_declaration")
        else {
            continue;
        };
        for p in list.named_children(&mut list.walk()) {
            let name = p.child_by_field_name("name").or_else(|| {
                let named = p.named_children(&mut p.walk()).last();
                named.filter(|l| matches!(l.kind(), "type_identifier" | "identifier"))
            });
            out.extend(name.map(|i| &src[i.byte_range()]));
        }
    }
    out
}

/// The names of the named namespaces (inline ones left out) around
/// `node`, outermost first; with `anonymous`, an anonymous namespace as
/// that segment, and the names of the classes, structs and unions too.
fn enclosing(node: Node, src: &str, anonymous: Option<&str>) -> Vec<String> {
    let mut path = Vec::new();
    for n in std::iter::successors(node.parent(), Node::parent) {
        let Some(name) = n.child_by_field_name("name") else {
            if let Some(a) = anonymous.filter(|_| n.kind() == "namespace_definition") {
                path.insert(0, a.to_string());
            }
            continue;
        };
        let name = &src[name.byte_range()];
        let classes = anonymous.is_some();
        match n.kind() {
            "namespace_definition" if !src[n.byte_range()].starts_with("inline") => {
                let segs = segments(name).filter(|s| !s.1);
                path.splice(0..0, segs.map(|s| s.0.to_string()));
            }
            "class_specifier" | "struct_specifier" | "union_specifier" if classes => {
                path.insert(0, name.to_string());
            }
            _ => {}
        }
    }
    path
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

/// The segments of a namespace name as written (`a::inline b`), each with
/// whether it names an inline namespace.
fn segments(name: &str) -> impl Iterator<Item = (&str, bool)> {
    let segs = name.split("::").map(|g| {
        let g = g.trim();
        let word = g
            .strip_prefix("inline")
            .filter(|r| r.starts_with(char::is_whitespace));
        word.map_or((g, false), |r| (r.trim(), true))
    });
    segs.filter(|g| !g.0.is_empty())
}

/// The function declarator under `node` that declares `name`, outside any
/// body.
fn declarator<'t>(node: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let mut stack = vec![node];
    loop {
        let n = stack.pop()?;
        let declares = n
            .child_by_field_name("declarator")
            .is_some_and(|d| src[d.byte_range()].ends_with(name));
        if n.kind() == "function_declarator" && declares {
            return Some(n);
        }
        if n.kind() != "compound_statement" {
            stack.extend(n.named_children(&mut n.walk()));
        }
    }
}

/// The parameter types of the function declarator `f`: each parameter's
/// text with its name, default value, whitespace, `struct`-like keywords
/// and a by-value parameter's `const` dropped, joined by `,`; `(void)` is
/// `()`.
fn params(f: Node, src: &str) -> Option<String> {
    let list = f.child_by_field_name("parameters")?;
    let mut out = Vec::new();
    for p in list.children(&mut list.walk()) {
        if matches!(p.kind(), "(" | ")" | "," | "comment") {
            continue;
        }
        let end = p
            .child_by_field_name("default_value")
            .map_or(p.end_byte(), |d| d.start_byte());
        // The parameter's own name: the innermost declarator.
        let mut d = p.child_by_field_name("declarator");
        while let Some(n) = d.filter(|n| n.kind() != "identifier") {
            d = n
                .child_by_field_name("declarator")
                .or_else(|| n.named_child(0));
        }
        let cut = d.map_or(end..end, |n| n.start_byte().min(end)..n.end_byte().min(end));
        let kept = [&src[p.start_byte()..cut.start], &src[cut.end..end]].concat();
        // `struct T*` and `T*` are one type in C++; so are `const T` and
        // `T` passed by value.
        // Words are dropped wherever they stand (`vector<enum E>`).
        let by_value = !kept.contains(['*', '&', '[', '(', '<']);
        let dropped = |w: &str| {
            matches!(w, "struct" | "class" | "union" | "enum")
                || (by_value && matches!(w, "const" | "volatile"))
        };
        let (mut typed, mut word) = (String::new(), String::new());
        for c in kept.chars().chain([' ']) {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
                continue;
            }
            if !dropped(&word) {
                typed.push_str(&word);
            }
            word.clear();
            if !c.is_whitespace() {
                typed.push(c);
            }
        }
        out.push(typed.trim_end_matches('=').to_string());
    }
    let all = out.join(",");
    Some(if all == "void" { String::new() } else { all })
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
