//! The facts a C/C++ file's trees show that its items do not.

use super::untemplated;
use crate::flow;
use crate::model::File;
use std::collections::BTreeSet;
use std::ops::Range;
use wiki_code::model::CodeItemKind;
use wiki_code::tree_sitter::Node;
use wiki_code::Analyzed;

/// What a C/C++ file's trees show that its items do not: which functions
/// are file-local, each function's parameter types and namespace, each
/// method's class, the file's classes and their bases, namespace names
/// and `using`s.
pub(super) fn cpp_facts(file: &mut File, a: &Analyzed, path: &str) {
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
            file.cpp.local.insert(i);
        }
        let f = declarator(node, &it.name, src.text());
        if let Some(p) = f.and_then(|f| params(f, src.text())) {
            file.cpp.params.insert(i, p);
        }
        // The enclosing named namespaces, then the qualifier of `a::B::f`.
        let declared = f.and_then(|f| f.child_by_field_name("declarator"));
        // A conversion operator's class is its qualifier, read first: the
        // type it names may be qualified too (`K::operator lib::T()`).
        let cast = decl.and_then(|d| cast_scope(d, src.text()));
        let qualifier = cast.or_else(|| Some(declared.and_then(|d| text(d).rsplit_once("::"))?.0));
        let segs = || qualifier.into_iter().flat_map(|q| q.split("::"));
        let scope = |inline: bool| {
            let mut path = enclosing(node, src.text(), None, inline);
            path.extend(segs().map(|s| s.trim().to_string()));
            path.retain(|s| !s.is_empty());
            path.join("::")
        };
        let (path, full) = (scope(false), scope(true));
        if full != path {
            file.cpp.inline_scopes.insert(i, full);
        }
        if !path.is_empty() {
            file.cpp.scopes.insert(i, path);
        }
        let in_class = std::iter::successors(node.parent(), Node::parent)
            .any(|n| n.kind() == "field_declaration_list");
        let names_owner =
            |o: &String| declared.is_some_and(|d| text(d).contains(&format!("{o}::")));
        let mut class = enclosing(node, src.text(), Some(&anon), false);
        if !in_class {
            class.extend(segs().map(|s| s.trim().to_string()));
        }
        let named = in_class || cast.is_some() || it.owner.as_ref().is_some_and(names_owner);
        if named && it.kind == CodeItemKind::Method {
            file.cpp.classes.insert(i, untemplated(&class.join("::")));
        }
    }
    for s in &a.sources {
        let mut stack = vec![s.tree().root_node()];
        while let Some(n) = stack.pop() {
            namespace(file, n, s.text());
            class_head(file, n, s.text(), &anon);
            alias(file, n, s.text(), &anon);
            specialization(file, n, s.text());
            variables(file, n, s.text());
            if n.kind() == "using_declaration" {
                using(file, n, s.text());
            }
            stack.extend(n.named_children(&mut n.walk()));
        }
    }
}

/// The class of the conversion operator `K::operator bool()` defined by
/// `d`: the qualifier before its `operator_cast` (`a::K` of
/// `a::K::operator int()`); or, when the grammar reads the definition
/// (after `inline`) as a function `bool` of the type `a::K::operator`,
/// which no type is named, `a::K`.
fn cast_scope<'s>(d: Node, src: &'s str) -> Option<&'s str> {
    let mut name = d.child_by_field_name("declarator");
    let start = name.map_or(0, |n| n.start_byte());
    while let Some(n) = name.filter(|n| n.kind() == "qualified_identifier") {
        name = n.child_by_field_name("name");
    }
    if let Some(cast) = name.filter(|n| n.kind() == "operator_cast" && n.start_byte() > start) {
        return Some(
            src[start..cast.start_byte()]
                .trim_end()
                .trim_end_matches("::"),
        );
    }
    let ty = d
        .child_by_field_name("type")
        .filter(|t| t.kind() == "qualified_identifier")?;
    let scope = src[ty.byte_range()].strip_suffix("operator")?.trim_end();
    scope.strip_suffix("::")
}

/// Records the names of the namespace or namespace alias `n` defines in
/// `CppFacts::namespaces`, a namespace's path in `CppFacts::namespace_paths`, and
/// an alias's namespace in `CppFacts::namespace_aliases`.
fn namespace(file: &mut File, n: Node, src: &str) {
    let alias = n.kind() == "namespace_alias_definition";
    let name = n.child_by_field_name("name");
    let Some(name) = name.filter(|_| alias || n.kind() == "namespace_definition") else {
        return;
    };
    file.cpp
        .namespaces
        .extend(segments(&src[name.byte_range()]).map(|g| g.0.to_string()));
    if !alias {
        let mut path = enclosing(n, src, None, false);
        let inline = src[n.byte_range()].starts_with("inline");
        for (seg, _) in segments(&src[name.byte_range()]).filter(|g| !(g.1 || inline)) {
            path.push(seg.to_string());
            file.cpp.namespace_paths.insert(path.join("::"));
        }
    }
    let named = n.named_children(&mut n.walk()).last();
    if let Some(t) = named.filter(|_| alias) {
        let t: String = src[t.byte_range()].split_whitespace().collect();
        let t = t.trim_start_matches("::").to_string();
        file.cpp
            .namespace_aliases
            .insert(src[name.byte_range()].to_string(), t);
    }
}

/// The class heads and function names the grammar misread in a file
/// (`class API Win : public Base {`, `class Win API {`, `API ret_t
/// f(…)`): the bytes of the two words of each, one of them a macro.
pub(super) fn misread_heads(a: &Analyzed) -> Vec<[Range<usize>; 2]> {
    let mut out = BTreeSet::new();
    for s in &a.sources {
        let mut stack = vec![s.tree().root_node()];
        while let Some(n) = stack.pop() {
            stack.extend(n.named_children(&mut n.walk()));
            let words = misread_head(n, s.text()).or_else(|| misread_name(n, s.text()));
            out.extend(words.map(|[a, b]| (a.start, a.end, b.start, b.end)));
        }
    }
    out.into_iter().map(|w| [w.0..w.1, w.2..w.3]).collect()
}

/// The words before a function's name when the grammar misread a macro
/// before its return type (`API ret_t f(…)` read as `ret_t::f` of the
/// type `API`, `API ret_t K::f(…)` as `ret_t K::f`): the declaration's
/// type and the scope that no `::` follows.
fn misread_name(n: Node, src: &str) -> Option<[Range<usize>; 2]> {
    let named = n
        .child_by_field_name("declarator")
        .filter(|_| n.kind() == "function_declarator");
    let named = named.filter(|d| d.kind() == "qualified_identifier")?;
    let scope = named.child_by_field_name("scope")?;
    let spaced = !src[scope.end_byte()..].trim_start().starts_with("::");
    let ty = n.parent()?.child_by_field_name("type");
    let ty = ty.filter(|t| spaced && t.kind() == "type_identifier")?;
    Some([ty.byte_range(), scope.byte_range()])
}

/// `n`'s two words when `n` is the `class A` of a head the grammar
/// misread: a class with no body, then a name, maybe `final`, and a base
/// clause or a body, in a broken declaration (one with an error, or a
/// function whose declarator is a bare name). `struct tm t{}` is none.
fn misread_head(n: Node, src: &str) -> Option<[Range<usize>; 2]> {
    let class = matches!(
        n.kind(),
        "class_specifier" | "struct_specifier" | "union_specifier"
    );
    let name = n.child_by_field_name("name");
    let name = name.filter(|m| class && m.kind() == "type_identifier");
    let name = name.filter(|_| n.child_by_field_name("body").is_none())?;
    let p = n.parent()?;
    let bare = p.kind() == "function_definition"
        && (p.child_by_field_name("declarator")).is_some_and(|d| d.kind() == "identifier");
    let broken = bare || p.has_error();
    let after = &src[n.end_byte()..];
    let start = n.end_byte() + after.len() - after.trim_start().len();
    let word = src[start..].find(|c: char| !(c.is_alphanumeric() || c == '_'));
    let end = start + word.filter(|&w| w > 0)?;
    let rest = src[end..].trim_start();
    let rest = rest.strip_prefix("final").map_or(rest, str::trim_start);
    let head = rest.starts_with('{') || (rest.starts_with(':') && !rest.starts_with("::"));
    (broken && head).then_some([name.byte_range(), start..end])
}

/// `text` with the macro of each misread head in `heads` blanked, when
/// one was told: the word a project file `#define`s, else the word in
/// capitals beside one that is not; with both or neither, none. The copy
/// has the same bytes and newlines, as the first-branch tree has.
pub(super) fn unmacroed(
    text: &str,
    heads: &[[Range<usize>; 2]],
    macros: &BTreeSet<String>,
) -> Option<String> {
    let caps = |w: &str| {
        w.contains(|c: char| c.is_ascii_uppercase())
            && w.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    };
    let mut out = text.as_bytes().to_vec();
    let mut any = false;
    for [one, other] in heads {
        let (a, b) = (&text[one.clone()], &text[other.clone()]);
        let pick = match (macros.contains(a), macros.contains(b)) {
            (true, false) => Some(one),
            (false, true) => Some(other),
            (true, true) => None,
            (false, false) => match (caps(a), caps(b)) {
                (true, false) => Some(one),
                (false, true) => Some(other),
                (true, true) | (false, false) => None,
            },
        };
        if let Some(r) = pick {
            out[r.clone()].fill(b' ');
            any = true;
        }
    }
    any.then(|| String::from_utf8(out).ok()).flatten()
}

/// Records the class head `n` (a class, struct or union with a body) in
/// `CppFacts::bases` and its data members in `CppFacts::fields`; `anonymous`
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
    let mut path = enclosing(n, src, Some(anonymous), false);
    path.push(text(name).to_string());
    let class = untemplated(&path.join("::"));
    let bases = bases.map(|b| (untemplated(text(b)), dependent(text(b))));
    file.cpp
        .bases
        .entry(class.clone())
        .or_default()
        .extend(bases);
    // A member the file's two trees type differently has no type.
    let fields = file.cpp.fields.entry(class).or_default();
    for (name, ty) in flow::cpp_fields(body, src, &params) {
        let kept = fields.entry(name).or_insert_with(|| ty.clone());
        if *kept != ty {
            *kept = None;
        }
    }
}

/// Records what the `using` declaration or directive `n` names in
/// `CppFacts::usings`, and by where it stands in `file_usings` or
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
        None => file.cpp.file_usings.insert(named.clone()),
        Some("field_declaration_list") => false,
        Some(_) => file.cpp.inner_usings.insert(named.clone()),
    };
    file.cpp.usings.insert(named);
}

/// Records in `CppFacts::variables` the names the declaration `n` outside any
/// function or class body declares as variables: each declarator's name
/// unless it declares a function, but one in parentheses (`(*say)(const
/// char*)`).
fn variables(file: &mut File, n: Node, src: &str) {
    if n.kind() != "declaration"
        || std::iter::successors(n.parent(), Node::parent)
            .any(|p| matches!(p.kind(), "compound_statement" | "field_declaration_list"))
    {
        return;
    }
    for mut d in n.children_by_field_name("declarator", &mut n.walk()) {
        loop {
            let inner = d
                .child_by_field_name("declarator")
                .or_else(|| d.named_child(0));
            match (d.kind(), inner) {
                ("identifier", _) => {
                    file.cpp.variables.insert(src[d.byte_range()].to_string());
                    break;
                }
                ("function_declarator", Some(i)) if i.kind() != "parenthesized_declarator" => break,
                (
                    "init_declarator"
                    | "array_declarator"
                    | "function_declarator"
                    | "pointer_declarator"
                    | "parenthesized_declarator"
                    | "reference_declarator",
                    Some(i),
                ) => d = i,
                _ => break,
            }
        }
    }
}

/// Records in `CppFacts::specialized` the function template the explicit
/// specialization `n` (`template <> int pack<Wide>(Wide v)`, defined or
/// declared) specializes.
fn specialization(file: &mut File, n: Node, src: &str) {
    let params = n.child_by_field_name("parameters");
    let empty = params.is_some_and(|p| p.named_child_count() == 0);
    if n.kind() != "template_declaration" || !empty {
        return;
    }
    let mut d = n.named_children(&mut n.walk()).last();
    d = d.filter(|d| matches!(d.kind(), "function_definition" | "declaration"));
    // Past a pointer or reference return type to the function declarator.
    while let Some(x) = d.and_then(|x| x.child_by_field_name("declarator")) {
        if x.kind() == "function_declarator" {
            let mut path = enclosing(n, src, None, false);
            let named = x.child_by_field_name("declarator");
            path.extend(named.map(|f| untemplated(&src[f.byte_range()])));
            file.cpp.specialized.insert(path.join("::"));
            return;
        }
        d = Some(x);
    }
}

/// Records the type alias `n` declares, if it is one outside any body,
/// in `CppFacts::aliases`.
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
    // A type naming a template parameter (`typename A::size_type`) is
    // known only per instance: none.
    let params = template_params(n, src);
    let ty = n.child_by_field_name("type");
    let ty = ty.map_or(String::new(), |t| match t.kind() {
        "struct_specifier" | "class_specifier" | "union_specifier" | "enum_specifier" => t
            .child_by_field_name("name")
            .map_or(String::new(), |n| untemplated(text(n))),
        _ => {
            let t = text(t).trim_start();
            let t = untemplated(t.strip_prefix("typename").unwrap_or(t));
            let t = t.trim_start_matches("::");
            let root = t.split("::").next().unwrap_or("");
            if params.contains(root) {
                String::new()
            } else {
                t.to_string()
            }
        }
    });
    for name in names.into_iter().filter(|_| !body) {
        let mut path = enclosing(n, src, Some(anonymous), false);
        path.push(name.to_string());
        // An alias the file's two trees read differently names nothing known.
        let kept = file.cpp.aliases.entry(untemplated(&path.join("::")));
        let kept = kept.or_insert_with(|| ty.clone());
        if *kept != ty {
            kept.clear();
        }
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

/// The names of the named namespaces (inline ones only with `inline`)
/// around `node`, outermost first; with `anonymous`, an anonymous
/// namespace as that segment, and the names of the classes, structs and
/// unions too.
fn enclosing(node: Node, src: &str, anonymous: Option<&str>, inline: bool) -> Vec<String> {
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
            "namespace_definition" if inline || !src[n.byte_range()].starts_with("inline") => {
                let segs = segments(name).filter(|s| inline || !s.1);
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
/// and a by-value parameter's `const` (a pointer's own too) dropped,
/// joined by `,`; `(void)` is
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
            // Two words keep a space between them (`const core::K`).
            if !dropped(&word) {
                if !word.is_empty() && typed.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                    typed.push(' ');
                }
                typed.push_str(&word);
            }
            word.clear();
            if !c.is_whitespace() {
                typed.push(c);
            }
        }
        // `T* const` and `T*` are one type passed by value.
        let typed = typed.trim_end_matches('=');
        let typed = (typed.strip_suffix("const"))
            .filter(|t| t.ends_with('*'))
            .unwrap_or(typed);
        out.push(typed.to_string());
    }
    let all = out.join(",");
    Some(if all == "void" { String::new() } else { all })
}
