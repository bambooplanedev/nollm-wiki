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
mod tests;
