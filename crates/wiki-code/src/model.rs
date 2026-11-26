//! The code model: the items an extractor finds in a source file and the calls between them.

use serde::{Deserialize, Serialize};

/// What a `CodeItem` is. Fields, enum variants and impl headers are not items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeItemKind {
    /// A free function.
    Fn,
    /// A function with an owner: an `impl` or class method, a trait method.
    Method,
    /// A struct, class, enum, trait, interface, union, type alias, or
    /// exported macro.
    Type,
    /// A constant, static, or module-level variable.
    Value,
    /// A module declaration.
    Module,
    /// A shell file's `<top level>`: the pseudo-caller of its calls outside
    /// every function. It is no symbol: search and the outline skip it; only
    /// `callers` shows it.
    #[serde(rename = "top_level")]
    TopLevel,
}

impl CodeItemKind {
    /// The serde name, as `items.json` and the outline print it.
    pub fn as_str(self) -> &'static str {
        match self {
            CodeItemKind::Fn => "fn",
            CodeItemKind::Method => "method",
            CodeItemKind::Type => "type",
            CodeItemKind::Value => "value",
            CodeItemKind::Module => "module",
            CodeItemKind::TopLevel => "top_level",
        }
    }
}

/// One definition of a code page and where it lives. Field order is the
/// `items.json` key order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeItem {
    pub kind: CodeItemKind,
    /// The name as the source writes it.
    pub name: String,
    /// The enclosing scopes, joined the way the language's signatures join
    /// them (`Wiki`, `<T as Display>`, `Outer.Inner`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The one-line signature as the source reads it, with no owner spliced
    /// in, so it can be searched for in the file. Exception: each C++
    /// declarator of a shared declaration gets its own signature, which may
    /// not appear verbatim (`int a, *b;` gives `b` the signature `int *b`).
    pub signature: String,
    /// 1-based, inclusive, in the original file: from the first attached doc
    /// comment or attribute line to the item's last line.
    pub start_line: usize,
    pub end_line: usize,
    /// The first sentence of the item's own doc comment, capped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// The item's whole doc, whitespace collapsed and capped; `None` when it
    /// equals `doc`. Scored at half weight, never shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_text: Option<String>,
    /// Whether the item is part of the page's export surface (`symbols`).
    pub public: bool,
    /// A Python function defined directly in another function's body, a
    /// shell file's top-level `NAME=` or a C/C++ data member (both a `Value`,
    /// matched by exact name only): found by `search` only, never in an
    /// outline, the exports or the call graph.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nested: bool,
    /// The `path` of the file holding this item's other half, when one is
    /// known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twin: Option<String>,
    /// Whether the item is test code: in a test file (`is_test_path`), inside
    /// a Rust `#[cfg(test)]` module, or a Rust `#[test]` function. Search
    /// shows such items but does not let them lift their page.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub test: bool,
    /// The functions this function calls, as `path:start-end`, sorted: the
    /// call sites in its body matched to project items by name (see
    /// `graph::resolve_calls`). Empty for anything but a function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<String>,
    /// For each entry of `calls`, the lines of this function that call it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_lines: Vec<Vec<usize>>,
    /// The call sites as the source names them, until `resolve_calls` turns
    /// them into `calls`.
    #[serde(skip)]
    pub call_sites: Vec<CallSite>,
    /// The argument counts a call to this function can pass, when the
    /// extractor knows them; `resolve_calls` drops a match that cannot fit.
    #[serde(skip)]
    pub arity: Option<Arity>,
    /// C++ only: which side of project-wide pairing this item may take.
    #[serde(skip)]
    pub pair_role: PairRole,
    /// C++ only: the innermost named, non-inline namespace around the
    /// item, its last segment (`lab::planner` gives `planner`).
    #[serde(skip)]
    pub namespace: Option<String>,
    /// C/C++ functions only: the parameter types as written, names,
    /// default values, `struct`-like keywords and whitespace but one space
    /// between two words dropped (`int,const char*`); `None` when no
    /// parameter list was found.
    #[serde(skip)]
    pub params: Option<String>,
}

/// C++ only: which side of project-wide pairing an item may take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PairRole {
    /// Not a candidate: other languages; inline or in-class bodies,
    /// `static` or anonymous-namespace definitions.
    #[default]
    Neither,
    /// `= delete`, or `= default` inside its class: never defined
    /// elsewhere, so it neither pairs nor counts toward a key's uniqueness.
    NoDefinition,
    /// A body-less declaration.
    Declaration,
    /// An out-of-line definition: a body or `= default`, no class-body
    /// ancestor, not `static`, not inside an anonymous namespace.
    OutOfLineDefinition,
}

/// The argument counts a function accepts: `min..=max`, `max` unbounded
/// for a variadic. `receiver` is a `self` that a `Type::f(x, …)` call
/// passes as the first argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arity {
    pub min: usize,
    pub max: Option<usize>,
    pub receiver: bool,
}

impl Arity {
    /// Whether a call passing `args` arguments can be a call of this
    /// function; `path` is a call through a type (`Q::f()`, Python
    /// `K.f()`), which may pass the receiver.
    pub fn fits(self, args: usize, path: bool) -> bool {
        let extra = usize::from(self.receiver && path);
        args >= self.min && self.max.is_none_or(|m| args <= m + extra)
    }
}

/// A call in a function body, as the source writes its target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    /// The called name: `f` of `f()`, `a::f()`, `x.f()`.
    pub name: String,
    pub via: CallVia,
    /// The arguments it passes; `None` when a spread or splat hides the count.
    pub args: Option<usize>,
    /// 1-based, in the original file.
    pub line: usize,
}

/// How a call site reaches its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallVia {
    /// `f()`.
    Bare,
    /// `q::f()`: the qualifier's last segment, generics dropped.
    Path(String),
    /// `self.f()`, `this->f()`.
    SelfRecv,
    /// `x.f()`: the receiver when it is a plain name.
    Member(Option<String>),
}

impl CodeItem {
    /// `path:start-end`, the item's lines in its source file.
    pub fn span(&self, path: &str) -> String {
        format!("{path}:{}-{}", self.start_line, self.end_line)
    }

    /// A gtest test (`TEST(Suite, Name)`, see `extract_cpp::gtest_names`):
    /// an `fn` owned by its suite whose signature is the macro call. No call
    /// names it, and its suite is no class its body's calls resolve against.
    pub fn is_test_case(&self) -> bool {
        self.kind == CodeItemKind::Fn
            && self.owner.is_some()
            && crate::formats::extract_cpp::GTEST_MACROS.iter().any(|m| {
                self.signature
                    .strip_prefix(m)
                    .is_some_and(|rest| rest.starts_with('('))
            })
    }

    /// The owner whose methods a call in this item's body may name bare:
    /// its owner, but none for a gtest test.
    pub fn call_owner(&self) -> Option<&str> {
        self.owner.as_deref().filter(|_| !self.is_test_case())
    }

    /// `owner::name`, or `name` when the item has no owner: how search
    /// prints the item, and a whole-name match for it.
    pub fn qualified_name(&self) -> String {
        match &self.owner {
            Some(o) => format!("{o}::{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Whether `rel_path` is a test file in any language: a path component
/// `tests`, or a file name `test_*`, `*_test.*`, `*_tests.*`.
pub fn is_test_path(rel_path: &str) -> bool {
    let mut parts = rel_path.split('/');
    let name = parts.next_back().unwrap_or("");
    let stem = name.split('.').next().unwrap_or("");
    parts.any(|c| c == "tests")
        || name.starts_with("test_")
        || (name.contains('.') && (stem.ends_with("_test") || stem.ends_with("_tests")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_item_kind_names_match_serde() {
        for k in [
            CodeItemKind::Fn,
            CodeItemKind::Method,
            CodeItemKind::Type,
            CodeItemKind::Value,
            CodeItemKind::Module,
            CodeItemKind::TopLevel,
        ] {
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        }
    }
}
