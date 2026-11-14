//! One pass over a source file for a consumer that needs more than the
//! items: the parse trees the items came from, the node each item was built
//! from, the calls under any node with their literal text, and comment text.
//!
//! Items' `call_sites` are attributed from the last source only, by line.
//! A C/C++ file whose braces the preprocessor splits has two sources and
//! can list one function twice, once per tree; the first tree's item then
//! has empty `call_sites`, though `calls_at` under its node yields calls.

use crate::formats::code::{self, CodeFile};
use crate::index::Family;
use crate::model::CallSite;
use std::collections::BTreeMap;
use tree_sitter::{Node, Tree};

/// A file's items together with the trees they were built from.
///
/// Each item's `call_sites` come from the last of `sources`, attributed by
/// line. With two sources (C/C++ with preprocessor-split braces) one
/// function can be listed twice, once per tree; the first-tree item then
/// has empty `call_sites` even though `calls_at` under its node yields calls.
#[derive(Clone, Debug)]
pub struct Analyzed {
    /// What `extract` returns for the same file.
    pub file: CodeFile,
    /// The trees the items came from, each with the text it indexes.
    pub sources: Vec<Source>,
    /// Parallel to `file.items`: the node an item was built from; `None`
    /// for an item with no single node (a shell file's `<top level>`, a
    /// shell function found by the text fallback).
    pub defs: Vec<Option<Def>>,
}

/// Where an item's node is: an index into `Analyzed::sources` and the
/// node's byte range in that source's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Def {
    pub source: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// One call under a node: the site an item gets for it and the byte range
/// of the call's own text in the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub site: CallSite,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// One tree the items came from, with the text its nodes index: the file's
/// text, or a component's script view (markup blanked in place). A shell
/// tree re-parsed from a blanked copy and a C/C++ tree of the first
/// preprocessor branch index the file's own text: those copies have its
/// length and lines, and the extraction reads the original bytes with them.
/// Items' `call_sites` are read from the last source only.
#[derive(Clone, Debug)]
pub struct Source {
    tree: Tree,
    text: String,
    aliases: BTreeMap<String, String>,
    family: Family,
}

impl Source {
    pub(crate) fn new(tree: Tree, text: &str, family: Family) -> Source {
        let aliases = code::import_aliases(tree.root_node(), text);
        Source {
            tree,
            text: text.to_string(),
            aliases,
            family,
        }
    }

    /// The parse tree.
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The text the tree's byte offsets index.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The family of the grammar the tree was parsed with.
    pub fn family(&self) -> Family {
        self.family
    }

    /// The node `def` points at. Where nodes nest on one range (a Python
    /// `expression_statement` around its `assignment`, a `block` holding one
    /// `function_definition`) the item was built from the innermost.
    pub fn node(&self, def: &Def) -> Option<Node<'_>> {
        self.tree
            .root_node()
            .descendant_for_byte_range(def.start_byte, def.end_byte)
            .filter(|n| n.byte_range() == (def.start_byte..def.end_byte))
    }

    /// The calls `node` itself makes, as its items' call sites record them
    /// (import aliases resolved); empty for a node that is not a call.
    /// `node` must belong to this source's tree: its byte offsets are read
    /// against this source's text and aliases. Under a first-tree node of a
    /// two-source file this yields calls its item's `call_sites` lack.
    pub fn calls_at(&self, node: Node) -> Vec<Call> {
        code::calls_at(
            node,
            &self.text,
            &self.aliases,
            self.family == Family::Shell,
        )
    }

    /// Whether `node` is a function definition in this source's grammar.
    pub fn is_function(&self, node: Node) -> bool {
        code::is_function(node)
    }

    /// Whether `node` is a comment (`line_comment`, `block_comment`, `comment`).
    pub fn is_comment(&self, node: Node) -> bool {
        code::is_comment_kind(node.kind())
    }

    /// A comment's text without its markers: `//`, `/* */` (a block's
    /// leading `*` too) or `#`.
    pub fn comment_text(&self, node: Node) -> String {
        let raw = self.text.get(node.byte_range()).unwrap_or("");
        match (self.family, raw.strip_prefix('#')) {
            (Family::Python, Some(body)) => {
                crate::formats::extract_python::comment_body(body).to_string()
            }
            (Family::Shell, Some(body)) => {
                crate::formats::extract_shell::comment_body(body).to_string()
            }
            _ => code::plain_comment_text(raw),
        }
    }
}

/// `None` when the file's extension has no grammar or the parser fails on
/// it; otherwise its items (exactly `extract`'s), the trees they came from
/// and each item's node. Items' `call_sites` come from the last source only
/// (see [`Analyzed`]).
pub fn analyze(rel_path: &str, text: &str) -> Option<Analyzed> {
    let (file, found) = code::extract_file(rel_path, text);
    let (sources, defs) = found?;
    Some(Analyzed {
        file,
        sources,
        defs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def_text(a: &Analyzed, name: &str) -> String {
        let i = a.file.items.iter().position(|it| it.name == name).unwrap();
        let d = a.defs[i].as_ref().unwrap();
        a.sources[d.source].text()[d.start_byte..d.end_byte].to_string()
    }

    #[test]
    fn every_function_item_points_at_its_own_node() {
        let a = analyze("src/a.rs", "fn a() {} fn a2() { g(); }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n").unwrap();
        assert_eq!(def_text(&a, "a"), "fn a() {}");
        assert_eq!(def_text(&a, "a2"), "fn a2() { g(); }");
        assert!(def_text(&a, "t").contains("fn t()")); // items of a stripped test module still have nodes
        assert_eq!(a.file, crate::extract("src/a.rs", a.sources[0].text()));
        assert!(analyze("notes.txt", "x").is_none());
    }

    #[test]
    fn defs_hold_in_every_family() {
        for (path, src, name, want) in [
            ("a.py", "def f(x):\n    return g(x)\n", "f", "def f(x):\n    return g(x)"),
            ("a.go", "package a\nfunc F(x int) int { return g(x) }\n", "F", "func F(x int) int { return g(x) }"),
            ("a.ts", "export function f(n: number) { return g(n) }\n", "f", "function f(n: number) { return g(n) }"),
            ("a.sh", "f() {\n  g \"$1\"\n}\nf x\n", "f", "f() {\n  g \"$1\"\n}"),
            ("a.cpp", "#if X\nint f(int a) { return g(a); }\n#else\nint h(int a) { return a; }\n#endif\n", "h", "int h(int a) { return a; }"),
            ("a.vue", "<template><div/></template>\n<script setup lang=\"ts\">\nfunction go(n: number) { emit(n) }\n</script>\n", "go", "function go(n: number) { emit(n) }"),
        ] {
            let a = analyze(path, src).unwrap();
            let i = a.file.items.iter().position(|it| it.name == name).unwrap();
            let d = a.defs[i].as_ref().unwrap();
            let s = &a.sources[d.source];
            assert!(s.is_function(s.node(d).unwrap()), "{path}");
            assert_eq!(def_text(&a, name), want, "{path}");
        }
    }

    #[test]
    fn a_function_split_by_the_preprocessor_has_a_def_in_each_tree() {
        let src = "#ifdef A\nint f(int a) {\n#else\nint f(long a) {\n#endif\n  return g(a);\n}\n";
        let a = analyze("a.cpp", src).unwrap();
        assert_eq!(a.sources.len(), 2);
        assert_eq!(a.defs.len(), a.file.items.len());
        let trees: Vec<usize> = a.defs.iter().flatten().map(|d| d.source).collect();
        assert!(trees.contains(&0) && trees.contains(&1), "{trees:?}");
    }

    #[test]
    fn shell_fallback_items_have_no_node() {
        let src = "FOO=1\nf() {\n echo\n}\nh() { ((( ; }\nBAZ=3\nf\n";
        let a = analyze("a.sh", src).unwrap();
        assert_eq!(a.defs.len(), a.file.items.len());
        let none: Vec<&str> = a
            .file
            .items
            .iter()
            .zip(&a.defs)
            .filter(|(_, d)| d.is_none())
            .map(|(it, _)| it.name.as_str())
            .collect();
        assert_eq!(none, ["h", "<top level>"]);
        let foo = a.file.items.iter().position(|it| it.name == "FOO").unwrap();
        assert!(a.defs[foo].is_some());
    }

    #[test]
    fn calls_at_gives_literal_text_also_inside_macros() {
        let src = "use a::b as c;\nfn f(x: u8) { g(h(x)); c::k(); let s = format!(\"{}\", crate::port(\"X\", 1)); }\n";
        let a = analyze("src/a.rs", src).unwrap();
        let s = &a.sources[0];
        let mut texts = Vec::new();
        let mut stack = vec![s.tree().root_node()];
        while let Some(n) = stack.pop() {
            let mut c = n.walk();
            stack.extend(n.named_children(&mut c));
            texts.extend(
                s.calls_at(n)
                    .into_iter()
                    .map(|c| s.text()[c.start_byte..c.end_byte].to_string()),
            );
        }
        texts.sort();
        assert_eq!(
            texts,
            ["c::k()", "crate::port(\"X\", 1)", "g(h(x))", "h(x)"]
        );
        // and they are the sites the item got
        assert_eq!(a.file.items[0].call_sites.len(), 4);
    }

    #[test]
    fn a_call_in_quoted_shell_code_points_at_its_own_text() {
        let src = "f() {\n  trap 'cleanup now' EXIT\n}\n";
        let a = analyze("a.sh", src).unwrap();
        let s = &a.sources[0];
        let mut texts = Vec::new();
        let mut stack = vec![s.tree().root_node()];
        while let Some(n) = stack.pop() {
            stack.extend(n.named_children(&mut n.walk()));
            for c in s.calls_at(n) {
                texts.push((c.site.name, s.text()[c.start_byte..c.end_byte].to_string()));
            }
        }
        texts.sort();
        assert_eq!(
            texts,
            [
                ("cleanup".to_string(), "cleanup now".to_string()),
                ("trap".to_string(), "trap 'cleanup now' EXIT".to_string()),
            ]
        );
    }

    #[test]
    fn comment_text_strips_the_markers_of_each_language() {
        for (path, src, want) in [
            ("a.rs", "// Tag. Why.\nfn f() {}\n", "Tag. Why."),
            ("a.py", "# Tag. Why.\ndef f():\n    pass\n", "Tag. Why."),
            ("a.sh", "# Tag. Why.\nf() { :; }\n", "Tag. Why."),
            (
                "a.cpp",
                "/* Tag. Why. */\nint f() { return 0; }\n",
                "Tag. Why.",
            ),
        ] {
            let a = analyze(path, src).unwrap();
            let s = &a.sources[0];
            let root = s.tree().root_node();
            let mut c = root.walk();
            let n = root
                .named_children(&mut c)
                .find(|n| s.is_comment(*n))
                .unwrap();
            assert_eq!(s.comment_text(n), want, "{path}");
        }
    }
}
