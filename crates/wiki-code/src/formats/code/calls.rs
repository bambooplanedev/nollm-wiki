//! Call-site extraction: attaches each call in a parse tree to its enclosing function item, with import aliases, arities and the shell and macro special cases.

use crate::analyze::{Call, Source};
use crate::index::Family;
use crate::model::{Arity, CallSite, CallVia, CodeItem, CodeItemKind};
use std::collections::BTreeMap;
use tree_sitter::{Node, Parser};

/// Gives each call in the tree to the narrowest function item whose lines
/// hold it. A call outside every function (a static initializer) is dropped,
/// except in a shell file: there it goes to the file's `<top level>` item,
/// pushed last. A shell call has no caller when a parse-ERROR node is closer
/// to it than any function definition. A nested function owns no calls: they
/// stay its enclosing function's.
pub(super) fn attach_call_sites(items: &mut Vec<CodeItem>, source: &Source) {
    let shell = source.family() == Family::Shell;
    let fns: Vec<usize> = (0..items.len())
        .filter(|&i| {
            matches!(items[i].kind, CodeItemKind::Fn | CodeItemKind::Method) && !items[i].nested
        })
        .collect();
    if fns.is_empty() && !shell {
        return;
    }
    let mut top_level: Vec<CallSite> = Vec::new();
    let mut stack = vec![source.tree().root_node()];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
        for Call { site, .. } in source.calls_at(node) {
            let line = site.line;
            let holder = fns
                .iter()
                .filter(|&&i| items[i].start_line <= line && line <= items[i].end_line)
                .min_by_key(|&&i| items[i].end_line - items[i].start_line);
            match holder {
                Some(&i) => items[i].call_sites.push(site),
                None if shell => top_level.push(site),
                None => {}
            }
        }
    }
    if !top_level.is_empty() {
        let lines = source.text().lines().count();
        let mut item = crate::formats::extract_shell::top_level_item(lines.max(1));
        item.call_sites = top_level;
        items.push(item);
    }
}

/// The calls `node` itself makes, each with the byte range of its own text:
/// a call node's, or for a call inside a Rust macro's token tree, from the
/// callee's first token to the end of its arguments. `aliases` is the file's
/// `import_aliases`. A shell call under a parse-ERROR node closer than any
/// function definition is not reported: the tree is not to be trusted for who
/// holds it.
pub(crate) fn calls_at(
    node: Node,
    text: &str,
    aliases: &BTreeMap<String, String>,
    shell: bool,
) -> Vec<Call> {
    // An alias a parameter or receiver of an enclosing function shadows
    // (Go's `func (m *T)` beside an import `m`) stays as written.
    let unalias = |n: &mut String, at: Node| {
        let Some(o) = aliases.get(n) else { return };
        let mut up = at.parent();
        while let Some(u) = up {
            let declares = |f: &str| {
                u.child_by_field_name(f).is_some_and(|p| {
                    text.get(p.byte_range())
                        .unwrap_or("")
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .any(|w| w == n)
                })
            };
            if declares("parameters") || declares("receiver") {
                return;
            }
            up = u.parent();
        }
        n.clone_from(o);
    };
    let call = |site: CallSite, start_byte: usize| Call {
        site,
        start_byte,
        end_byte: node.end_byte(),
    };
    // `call` is Python's; every other grammar says `call_expression`.
    let mut calls: Vec<Call> = match node.kind() {
        "call_expression" | "call" => node
            .child_by_field_name("function")
            .and_then(|f| call_site(f, text))
            .map(|mut s| {
                s.args = call_args(node);
                call(s, node.start_byte())
            }),
        "token_tree" => macro_call_site(node, text).map(|(s, start)| call(s, start)),
        "command" if shell => shell_call_site(node, text).map(|s| call(s, node.start_byte())),
        _ => None,
    }
    .into_iter()
    .collect();
    for Call { site, .. } in &mut calls {
        match &mut site.via {
            CallVia::Bare => unalias(&mut site.name, node),
            CallVia::Path(q) | CallVia::Member(Some(q)) => unalias(q, node),
            _ => {}
        }
        site.line = node.start_position().row + 1;
    }
    if shell && node.kind() == "command" {
        calls.extend(shell_string_calls(node, text));
    }
    // An ERROR node met before any function definition: the tree is not
    // to be trusted for who holds this call.
    if shell
        && !calls.is_empty()
        && std::iter::successors(node.parent(), Node::parent)
            .find(|a| a.is_error() || a.kind() == "function_definition")
            .is_some_and(|a| a.is_error())
    {
        return Vec::new();
    }
    calls
}

/// A shell `command` whose word is a plain name; a variable, a path or a
/// quoted word names no function. The arguments are not counted.
fn shell_call_site(node: Node, text: &str) -> Option<CallSite> {
    let name = text.get(node.child_by_field_name("name")?.byte_range())?;
    name.chars()
        .all(|c| c.is_alphanumeric() || "_:.-+@".contains(c))
        .then(|| CallSite {
            name: name.to_string(),
            via: CallVia::Bare,
            args: None,
            line: 0,
        })
}

/// The commands in the quoted code a `trap`, `eval` or `sh -c` command
/// runs, each a call at its own line and range; a command substitution in it
/// is already one in the outer tree and is skipped.
fn shell_string_calls(node: Node, text: &str) -> Vec<Call> {
    let word = |n: &Node| text.get(n.byte_range()).unwrap_or("");
    let mut cursor = node.walk();
    let args: Vec<Node> = node
        .children_by_field_name("argument", &mut cursor)
        .collect();
    let code = match node.child_by_field_name("name").map(|n| word(&n)) {
        Some("trap" | "eval") => args.first(),
        Some("bash" | "sh" | "zsh") => args
            .iter()
            .position(|a| {
                word(a).starts_with('-') && !word(a).starts_with("--") && word(a).contains('c')
            })
            .and_then(|i| args.get(i + 1)),
        _ => None,
    };
    let Some(code) = code.filter(|a| matches!(a.kind(), "raw_string" | "string")) else {
        return Vec::new();
    };
    let inner = word(code);
    let inner = inner.get(1..inner.len().saturating_sub(1)).unwrap_or("");
    let mut parser = Parser::new();
    let Some(tree) = parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .ok()
        .and_then(|()| parser.parse(inner, None))
    else {
        return Vec::new();
    };
    let mut sites = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.kind() == "command_substitution" {
            continue;
        }
        let mut c = n.walk();
        stack.extend(n.named_children(&mut c));
        if n.kind() == "command" {
            if let Some(mut site) = shell_call_site(n, inner) {
                site.line = code.start_position().row + n.start_position().row + 1;
                // `inner` starts one byte, the opening quote, into `code`.
                let at = code.start_byte() + 1;
                sites.push(Call {
                    site,
                    start_byte: at + n.start_byte(),
                    end_byte: at + n.end_byte(),
                });
            }
        }
    }
    sites
}

/// What a call's `function` node names, in any grammar this file dispatches
/// on; `None` for a call through an expression (`f()()`, `(*p)()`), an
/// operator or a destructor.
fn call_site(f: Node, text: &str) -> Option<CallSite> {
    let field = |name: &str| f.child_by_field_name(name);
    let text_of = |n: Node| text.get(n.byte_range());
    match f.kind() {
        "identifier" => Some(CallSite {
            name: text_of(f)?.to_string(),
            via: CallVia::Bare,
            args: None,
            line: 0,
        }),
        "generic_function" => call_site(field("function")?, text),
        "template_function" => call_site(field("name")?, text),
        // Rust `a::b::f`, C++ `a::b::f` (whose `name` nests the rest).
        "scoped_identifier" | "qualified_identifier" => {
            let inner = call_site(field("name")?, text)?;
            if inner.via != CallVia::Bare {
                return Some(inner);
            }
            let Some(scope) = field("path").or_else(|| field("scope")) else {
                return Some(inner);
            };
            // `Vec::<u8>` and `Foo<T>` are `Vec` and `Foo`.
            let scope = text_of(scope)?.split('<').next().unwrap_or("");
            let last = scope
                .trim_end_matches("::")
                .rsplit("::")
                .next()
                .unwrap_or("");
            let last = last.trim();
            Some(CallSite {
                via: CallVia::Path(last.to_string()),
                ..inner
            })
        }
        "field_expression" | "member_expression" | "attribute" | "selector_expression" => {
            let mut name = field("field")
                .or_else(|| field("property"))
                .or_else(|| field("attribute"))?;
            if name.kind() == "template_method" {
                name = name.child_by_field_name("name")?;
            }
            if !matches!(
                name.kind(),
                "field_identifier" | "property_identifier" | "identifier"
            ) {
                return None;
            }
            let recv = field("value")
                .or_else(|| field("argument"))
                .or_else(|| field("object"))
                .or_else(|| field("operand"))?;
            let recv_text = text_of(recv)?;
            let via = match recv.kind() {
                "self" | "this" => CallVia::SelfRecv,
                "identifier" if recv_text == "self" => CallVia::SelfRecv,
                "identifier" => CallVia::Member(Some(recv_text.to_string())),
                _ => CallVia::Member(None),
            };
            Some(CallSite {
                name: text_of(name)?.to_string(),
                via,
                args: None,
                line: 0,
            })
        }
        _ => None,
    }
}

/// A file's import aliases, each local name to the name it stands for, one
/// map for the whole file: `import { f as g }`, `from m import f as g` and
/// `use m::f as g` give `g` → `f`; a module alias (`import * as m from
/// './a/b'`, `import a.b as m`, `use a::b::{self as m}`, Go's `m "a/b"`)
/// gives `m` → `b`, the module's last segment, past an `index` file or a Go
/// `/vN` suffix.
pub(crate) fn import_aliases(root: Node, text: &str) -> BTreeMap<String, String> {
    let text_of = |n: Node| text.get(n.byte_range()).unwrap_or("");
    let last = |s: &str| {
        let segment = match s.strip_prefix(['"', '\'', '`']) {
            Some(path) => path
                .trim_end_matches(['"', '\'', '`'])
                .rsplit('/')
                .map(|f| f.split('.').next().unwrap_or(""))
                .find(|f| {
                    let version = f.len() > 1
                        && f.starts_with('v')
                        && f[1..].bytes().all(|b| b.is_ascii_digit());
                    *f != "index" && !version
                }),
            None => s.rsplit(['.', ':']).next(),
        };
        segment.unwrap_or("").to_string()
    };
    let mut out = BTreeMap::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
        let field = |f: &str| node.child_by_field_name(f);
        let pair = match node.kind() {
            "import_specifier" | "aliased_import" => {
                field("alias").zip(field("name").map(|n| last(text_of(n))))
            }
            // `a::b::{self as m}` names `b`, the list's path.
            "use_as_clause" => field("alias").zip(field("path").map(|n| {
                match text_of(n) {
                    "self" => node
                        .parent()
                        .and_then(|l| l.parent())
                        .and_then(|u| u.child_by_field_name("path"))
                        .map_or_else(String::new, |p| last(text_of(p))),
                    t => last(t),
                }
            })),
            "import_spec" => field("name").zip(field("path").map(|n| last(text_of(n)))),
            // The source string sits on the `import_statement` two levels up.
            "namespace_import" => node.named_child(0).zip(
                node.parent()
                    .and_then(|c| c.parent())
                    .and_then(|s| s.child_by_field_name("source"))
                    .map(|s| last(text_of(s))),
            ),
            // C++ `namespace d = a::b;` names `b`.
            "namespace_alias_definition" => {
                field("name").zip(node.named_child(1).map(|n| last(text_of(n))))
            }
            _ => None,
        };
        if let Some((alias, orig)) = pair {
            let alias = text_of(alias);
            if !matches!(orig.as_str(), "" | "default" | "self")
                && !matches!(alias, "_" | ".")
                && alias != orig
            {
                out.insert(alias.to_string(), orig);
            }
        }
    }
    out
}

/// A call inside a Rust macro's arguments (`format!("{}", f(x))`,
/// `select! { v = rx.recv() => … }`), which the grammar keeps as bare
/// tokens: a `(…)` token tree right after a name, read as `f(…)`, `q::f(…)`
/// or `x.f(…)`, generic arguments skipped. Attribute arguments,
/// `macro_rules!` bodies and the names an item inside a macro defines
/// (`fn f(…)`, `struct P(…)`) are not calls. Beside the site: where the
/// call's text starts, the first token of its `q::f` / `x.f` chain.
fn macro_call_site(tree: Node, text: &str) -> Option<(CallSite, usize)> {
    // The token before `n`, stepping over `<…>` and the turbofish `::` before it.
    fn back<'t>(n: Node<'t>, text: &str) -> Option<Node<'t>> {
        let text_of = |n: Node| text.get(n.byte_range()).unwrap_or("");
        let mut p = n.prev_sibling()?;
        if text_of(p).bytes().all(|b| b == b'>') {
            let (mut close, mut open) = (0, 0);
            loop {
                let t = text_of(p);
                if t.bytes().all(|b| b == b'<' || b == b'>') {
                    close += t.matches('>').count();
                    open += t.matches('<').count();
                }
                if open >= close {
                    break;
                }
                p = p.prev_sibling()?;
            }
            p = p.prev_sibling()?;
            if text_of(p) == "::" {
                p = p.prev_sibling()?;
            }
        }
        Some(p)
    }
    let text_of = |n: Node| text.get(n.byte_range()).unwrap_or("");
    let mut up = tree.parent()?;
    while up.kind() == "token_tree" {
        up = up.parent()?;
    }
    if up.kind() != "macro_invocation" || !text_of(tree).starts_with('(') {
        return None;
    }
    let name = back(tree, text).filter(|n| n.kind() == "identifier")?;
    let word = |n: Node| matches!(n.kind(), "identifier" | "self" | "crate" | "super");
    let via = match back(name, text) {
        Some(k)
            if matches!(
                text_of(k),
                "fn" | "struct" | "enum" | "union" | "trait" | "type"
            ) =>
        {
            return None;
        }
        Some(sep) if text_of(sep) == "::" => {
            CallVia::Path(text_of(back(sep, text).filter(|q| word(*q))?).to_string())
        }
        Some(sep) if text_of(sep) == "." => {
            // A plain name before the dot is the receiver; `a.b.f()` has none.
            let recv = back(sep, text).filter(|w| {
                word(*w) && back(*w, text).is_none_or(|b| !matches!(text_of(b), "." | "::"))
            });
            match recv.map(text_of) {
                Some("self") => CallVia::SelfRecv,
                Some(w) => CallVia::Member(Some(w.to_string())),
                None => CallVia::Member(None),
            }
        }
        _ => CallVia::Bare,
    };
    // Arguments are the commas at the top level of the tree, plus one; a
    // closure's `|a, b|` or a `<A, B>` there hides the count.
    let mut cursor = tree.walk();
    let inner: Vec<Node> = tree.children(&mut cursor).collect();
    let inner = inner.get(1..inner.len().saturating_sub(1)).unwrap_or(&[]);
    let unsure = inner
        .iter()
        .any(|n| matches!(text_of(*n), "|" | "||" | "<"));
    let commas = inner.iter().filter(|n| text_of(**n) == ",").count();
    let trailing = inner.last().is_some_and(|n| text_of(*n) == ",");
    let args = match inner {
        _ if unsure => None,
        [] => Some(0),
        _ => Some(commas + 1 - usize::from(trailing)),
    };
    // The callee's text starts at the first word of its `::` / `.` chain.
    let mut start = name;
    while let Some(w) = back(start, text)
        .filter(|s| matches!(text_of(*s), "::" | "."))
        .and_then(|s| back(s, text))
        .filter(|w| word(*w))
    {
        start = w;
    }
    Some((
        CallSite {
            name: text_of(name).to_string(),
            via,
            args,
            line: 0,
        },
        start.start_byte(),
    ))
}

/// How many arguments a call passes; `None` when a spread, splat or pack
/// expansion hides the count.
fn call_args(call: Node) -> Option<usize> {
    let args = call.child_by_field_name("arguments")?;
    // Python `f(x for x in xs)`: the generator is the one argument.
    if args.kind() == "generator_expression" {
        return Some(1);
    }
    let mut n = 0;
    for a in args.named_children(&mut args.walk()) {
        match a.kind() {
            "list_splat"
            | "dictionary_splat"
            | "spread_element"
            | "variadic_argument"
            | "parameter_pack_expansion" => return None,
            k if k.contains("comment") || k == "attribute_item" => {}
            _ => n += 1,
        }
    }
    Some(n)
}

/// The argument counts a function definition accepts, read off its
/// parameter list. `None` where a count proves nothing: in JS/TS any count
/// is legal, and a Python decorator other than `staticmethod` or
/// `classmethod` may change the signature. A C++ function gets no minimum,
/// since a default can sit on its other declaration alone. A Python method
/// takes its first parameter as the receiver, whatever its name.
pub(super) fn arity_of(def: Node, lang: &str, method: bool, text: &str) -> Option<Arity> {
    let mut receiver = lang == "go" && def.kind() == "method_declaration";
    let params = match lang {
        "rust" | "go" => def.child_by_field_name("parameters")?,
        "python" => {
            let decorators: Vec<&str> = def
                .parent()
                .filter(|p| p.kind() == "decorated_definition")
                .map(|p| {
                    p.named_children(&mut p.walk())
                        .filter(|c| c.kind() == "decorator")
                        .filter_map(|c| text.get(c.byte_range()))
                        .map(|d| d.trim_start_matches('@').trim())
                        .collect()
                })
                .unwrap_or_default();
            if decorators
                .iter()
                .any(|d| !matches!(*d, "staticmethod" | "classmethod"))
            {
                return None;
            }
            receiver = method && !decorators.contains(&"staticmethod");
            def.child_by_field_name("parameters")?
        }
        "cpp" => cpp_parameters(def)?,
        _ => return None,
    };
    let (mut min, mut max) = (0, Some(0));
    // A Python receiver is the first parameter of any shape.
    let mut skip_first = lang == "python" && receiver;
    for p in params.named_children(&mut params.walk()) {
        let k = p.kind();
        if k.contains("comment")
            || matches!(
                k,
                "attribute_item" | "keyword_separator" | "positional_separator"
            )
        {
            continue;
        }
        if std::mem::take(&mut skip_first) {
            continue;
        }
        let n = match k {
            "self_parameter" => {
                receiver = true;
                continue;
            }
            "variadic_parameter"
            | "variadic_parameter_declaration"
            | "list_splat_pattern"
            | "dictionary_splat_pattern" => {
                max = None;
                continue;
            }
            // Python `*xs: T` and `**kw: T`.
            "typed_parameter"
                if p.named_child(0).is_some_and(|c| {
                    matches!(c.kind(), "list_splat_pattern" | "dictionary_splat_pattern")
                }) =>
            {
                max = None;
                continue;
            }
            "default_parameter" | "typed_default_parameter" | "optional_parameter_declaration" => {
                max = max.map(|m| m + 1);
                continue;
            }
            "parameter_declaration" if text.get(p.byte_range()) == Some("void") => 0,
            // Go `a, b int` declares two.
            "parameter_declaration" if lang == "go" => p
                .children_by_field_name("name", &mut p.walk())
                .count()
                .max(1),
            _ => 1,
        };
        min += n;
        max = max.map(|m| m + n);
    }
    if lang == "cpp" {
        min = 0;
    }
    Some(Arity { min, max, receiver })
}

/// A C++ function's parameter list: under its function declarator, through
/// any pointer or reference declarator wrapping it. A definition's is the
/// one naming the function, not that of a function pointer it returns.
fn cpp_parameters(def: Node) -> Option<Node> {
    let mut d = def.child_by_field_name("declarator")?;
    if def.kind() == "function_definition" {
        d = crate::formats::extract_cpp::naming_declarator(d).unwrap_or(d);
    }
    while d.kind() != "function_declarator" {
        let mut cursor = d.walk();
        let inner = d.child_by_field_name("declarator").or_else(|| {
            d.named_children(&mut cursor)
                .find(|c| c.kind().ends_with("declarator"))
        });
        d = inner?;
    }
    d.child_by_field_name("parameters")
}

/// Whether a definition node is a function, in any grammar this file
/// dispatches on. A C++ header states its members as declarations whose
/// declarator is a function declarator — `void apply(const Order&);` is a
/// `field_declaration`, not a `function_definition`; the declarator test goes
/// through `extract_cpp::is_function_declarator`, which also accepts the
/// pointer/reference wrapper a member returning `Level*` or `Level&` puts in
/// the way. A JS/TS `const f = () => …` is a function through its value.
///
/// A `declaration` sharing several declarators (`int x, f();`) is judged by
/// its first, so it is a value there though a later declarator is a function.
///
/// Shared by `methods` and item kinds. The JS/TS/Go kinds never arrive as a
/// kept member `@def`, so they leave `methods` as it was.
pub(crate) fn is_function(def: Node) -> bool {
    match def.kind() {
        "function_item"
        | "function_signature_item"
        | "function_definition"
        | "function_declaration"
        | "generator_function_declaration"
        | "method_definition"
        | "method_declaration" => true,
        "field_declaration" | "declaration" => def
            .child_by_field_name("declarator")
            .is_some_and(crate::formats::extract_cpp::is_function_declarator),
        "variable_declarator" => def
            .child_by_field_name("value")
            .is_some_and(|v| matches!(v.kind(), "arrow_function" | "function_expression")),
        _ => false,
    }
}
