//! The data every stage passes along.

use std::collections::{BTreeMap, BTreeSet};
use wiki_code::index::Family;
use wiki_code::model::{CallSite, CodeItem};
use wiki_code::{CodeFile, CodeUnit};

/// Every analysed file, keyed by path relative to the root (`/` separators).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub files: BTreeMap<String, File>,
    /// Relative paths of code files that could not be read or could not be
    /// parsed at all.
    pub unreadable: Vec<String>,
    /// The project's own Rust crates as `(name, directory)`: the `[package]`
    /// and `[lib]` names of the tree's `Cargo.toml` files, `-` read as `_`,
    /// and the manifest's directory (`""` at the top of the tree).
    pub crates: BTreeSet<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub code: CodeFile,
    pub family: Family,
    /// Keyed by index into `code.items`.
    pub flows: BTreeMap<usize, Flow>,
    /// True when any of the file's trees has an error node.
    pub syntax_errors: bool,
    /// The file's line count, at least 1.
    pub lines: usize,
    /// Python's imports outside any function, read from the tree; empty for
    /// the other languages, whose import strings are `code.imports`.
    pub imports: Vec<Import>,
    /// Functions left out of `flows` as test code.
    pub test_fns: usize,
    /// Python: a call or a decision at module level, which no flow shows.
    pub module_code: bool,
}

/// One name a Python import statement imports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// As written: `a.b`, `.m`, `.`.
    pub module: String,
    /// `f` of `from m import f`, `*` of a glob; `None` for `import a.b`.
    pub name: Option<String>,
    /// `g` of `… as g`.
    pub alias: Option<String>,
}

impl Import {
    /// The name the import binds: the alias, the imported name, or the
    /// first segment of `import a.b`; `None` for a glob.
    pub fn binds(&self) -> Option<&str> {
        match (&self.alias, &self.name) {
            (Some(a), _) => Some(a),
            (None, Some(n)) => (n != "*").then_some(n.as_str()),
            (None, None) => self.module.split('.').next(),
        }
    }

    /// The module path the import names: `a.b` of `import a.b` and of
    /// `from a import b`, `.m` of `from . import m`.
    pub fn path(&self) -> String {
        match &self.name {
            Some(n) if self.module.ends_with('.') => format!("{}{n}", self.module),
            Some(n) => format!("{}.{n}", self.module),
            None => self.module.clone(),
        }
    }
}

impl CodeUnit for File {
    fn path(&self) -> &str {
        self.code.path()
    }
    fn lang(&self) -> Option<&str> {
        self.code.lang()
    }
    fn items(&self) -> &[CodeItem] {
        self.code.items()
    }
    fn items_mut(&mut self) -> &mut Vec<CodeItem> {
        self.code.items_mut()
    }
    fn defined(&self) -> &[String] {
        self.code.defined()
    }
}

/// Constructs the flow does not draw, counted per function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotDrawn {
    Try,
    UnwrapOr,
    Closure,
    Ternary,
    TryBlock,
    Goto,
    IfDef,
    ShortCircuit,
}

impl NotDrawn {
    pub fn label(self) -> &'static str {
        match self {
            NotDrawn::Try => "?",
            NotDrawn::UnwrapOr => "unwrap_or",
            NotDrawn::Closure => "closure",
            NotDrawn::Ternary => "ternary",
            NotDrawn::TryBlock => "try",
            NotDrawn::Goto => "goto",
            NotDrawn::IfDef => "#if",
            NotDrawn::ShortCircuit => "&&/||",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flow {
    pub steps: Vec<Step>,
    pub not_drawn: BTreeMap<NotDrawn, usize>,
    pub locals: BTreeSet<String>,
    pub types: BTreeMap<String, String>,
    /// Names that mean something else here than in the file: the generic
    /// parameters in scope and the items declared in the body.
    pub rebound: BTreeSet<String>,
    pub syntax_errors: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Branch {
        head: Option<String>,
        line: usize,
        arms: Vec<Arm>,
        /// No way past the last arm (a Rust `match`).
        exhaustive: bool,
    },
    Loop {
        header: String,
        line: usize,
        comment: Option<Comment>,
        /// The header's steps when it runs before every iteration (`while`).
        pre: Vec<Step>,
        body: Vec<Step>,
        /// `'outer` of `'outer: for …`.
        label: Option<String>,
        /// Left only by a `break` (Rust's header-less `loop`).
        endless: bool,
        /// A macro in the body, in no inner loop, has a `break` token: it
        /// may leave the loop unseen.
        hidden_exit: bool,
        /// Run when the loop ends without a `break` (Python's loop `else`).
        orelse: Vec<Step>,
    },
    Call(MapCall),
    Exit {
        text: String,
        line: usize,
        kind: ExitKind,
    },
    Assign {
        target: String,
        value: String,
        line: usize,
    },
    Nested {
        name: String,
        line: usize,
        flow: Flow,
    },
    /// The body of a `finally`, after the steps of its `try`: on every path
    /// that falls into it. When none does (each leaves by an exit, which
    /// runs it on the way out), it is reached only by edges not drawn.
    Finally {
        line: usize,
        body: Vec<Step>,
    },
}

/// Where an exit goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitKind {
    /// Leaves the function: `return`, a panic.
    Return,
    /// Leaves the loop with this label, or the innermost one.
    Break(Option<String>),
    /// Back to the loop with this label, or the innermost one.
    Continue(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arm {
    /// `None` is the else arm.
    pub cond: Option<String>,
    pub line: usize,
    pub comment: Option<Comment>,
    /// Calls in the condition.
    pub pre: Vec<Step>,
    pub body: Vec<Step>,
    /// Run right after `body`, written after every other arm: the `else`
    /// of Python's `try`.
    pub after: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub text: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapCall {
    pub text: String,
    pub site: CallSite,
    /// First segment of a multi-segment path.
    pub root: Option<String>,
    pub target: Target,
}

/// A handle valid within one build, never written to a page.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FnId {
    pub path: String,
    pub item: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Target {
    Resolved(FnId),
    OneOf {
        candidates: Vec<FnId>,
        outside: bool,
    },
    Outside,
    #[default]
    Hidden,
}
