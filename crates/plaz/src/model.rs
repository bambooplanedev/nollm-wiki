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
    /// Relative paths of the Objective-C, Objective-C++ and CUDA source
    /// files (`.m`, `.mm`, `.cu`): no grammar reads them, but they may
    /// define what a C/C++ header of the same stem declares.
    pub twins: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub code: CodeFile,
    pub family: Family,
    /// Keyed by index into `code.items`.
    pub flows: BTreeMap<usize, Flow>,
    /// The C/C++ function items with no body to chart, by index into
    /// `code.items`: a section each, with no flow.
    pub declarations: BTreeMap<usize, Bodiless>,
    /// True when any of the file's trees has an error node.
    pub syntax_errors: bool,
    /// The file's line count, at least 1.
    pub lines: usize,
    /// Python's imports outside any function, read from the tree; empty for
    /// the other languages, whose import strings are `code.imports`.
    pub imports: Vec<Import>,
    /// Functions left out of `flows` as test code.
    pub test_fns: usize,
    /// A call or a decision at module level (Python), a declaration
    /// outside any function initialized by a call (C/C++): no flow shows it.
    pub module_code: bool,
    /// Every `#define` name of a C/C++ file, once, sorted, and whether a
    /// body of it holds a `return`, `goto`, `break`, `continue` or `throw`
    /// token; empty for the other languages.
    pub macros: Vec<(String, bool)>,
    /// The C/C++ function items, by index into `code.items`, that are
    /// `static` free functions or inside an anonymous namespace: seen only
    /// where the file is compiled in.
    pub local: BTreeSet<usize>,
    /// Per C/C++ function item: its parameter types as written, names,
    /// default values, `struct` and whitespace but one space between two
    /// words dropped (`int,const char*`).
    pub params: BTreeMap<usize, String>,
    /// Every segment of a C/C++ file's named namespaces and every namespace
    /// alias it defines.
    pub namespaces: BTreeSet<String>,
    /// Per namespace alias a C/C++ file defines: the namespace it names,
    /// whitespace and a leading `::` dropped (`fs` → `std::filesystem`).
    pub namespace_aliases: BTreeMap<String, String>,
    /// Per C/C++ function item in a named namespace or with a qualified
    /// name: the enclosing namespaces, then the qualifier (`a::B` of
    /// `void a::B::f()` in no namespace), `::`-joined; anonymous and inline
    /// namespaces left out.
    pub scopes: BTreeMap<usize, String>,
    /// What each `using namespace N;` and `using N::f;` of a C/C++ file
    /// names: `N`, `N::f`, whitespace and a leading `::` dropped.
    pub usings: BTreeSet<String>,
    /// The `usings` that stand at file scope: in no function body, class
    /// body or named namespace.
    pub file_usings: BTreeSet<String>,
    /// The other `usings`, but those in a class body (they bring members,
    /// such as inherited constructors).
    pub inner_usings: BTreeSet<String>,
    /// Per C/C++ method item, by index into `code.items`: its class as a
    /// path (`n::Outer::Inner`), template arguments and whitespace dropped
    /// (`untemplated`), an anonymous namespace a segment of its own per
    /// file: the enclosing namespaces and classes of a method in a class
    /// body, those and the declarator's qualifier for one defined outside.
    /// None when the declarator does not name the owner (a macro before
    /// the return type made the return type the owner).
    pub classes: BTreeMap<usize, String>,
    /// Per C/C++ class, struct or union the file defines, by path as in
    /// `classes`: the bases its heads name, as written, `untemplated`, each
    /// with whether its template arguments name a template parameter of
    /// the class or of a class around it (a dependent base).
    pub bases: BTreeMap<String, BTreeSet<(String, bool)>>,
    /// Every C/C++ type alias the file declares outside function bodies
    /// (`using X = …`, an alias template, `typedef … X`), by path as in
    /// `classes`, with the type it names as written, template arguments,
    /// whitespace, `typename` and a leading `::` dropped (`std::vector` of
    /// `typedef std::vector<int> Ints`); `""` for an unnamed class, a type
    /// naming a template parameter, or two readings that differ; a
    /// `typedef struct X X` is the class, no alias.
    pub aliases: BTreeMap<String, String>,
    /// Per C/C++ class the file defines, by path as in `bases`: each data
    /// member its body declares, with its written type as a local's is
    /// read (`Flow::types`); none for a function pointer, a type naming a
    /// template parameter of the class, or one the file's trees type
    /// differently.
    pub fields: BTreeMap<String, BTreeMap<String, Option<String>>>,
    /// The variables a C/C++ file declares outside function and class
    /// bodies, function pointers included (`say` of `static void
    /// (*say)(const char*);`): a call of one is no function's.
    pub variables: BTreeSet<String>,
    /// The function templates a C/C++ file explicitly specializes, whose
    /// specializations no call is matched to: each by its path, the enclosing
    /// named namespaces then the name as written, template arguments
    /// dropped (`a::pk::pack` of `template <> int pk::pack<Wide>(Wide)` in
    /// `namespace a`).
    pub specialized: BTreeSet<String>,
}

/// What a C/C++ function item has in place of a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bodiless {
    /// A declaration: `void f(int);`, a pure virtual.
    Declared,
    /// `= default`.
    Defaulted,
    /// `= delete`.
    Deleted,
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
    ShortCircuit,
    Goto,
    /// A conditional directive group drawn as if unconditional.
    Preprocessor,
    /// A call with no call site: `new T(…)`, a call through an expression.
    Untraced,
    /// A `case` label inside a statement in a switch (Duff's device): no
    /// edge enters it.
    NestedCase,
    /// A statement in a switch before its first label: no arm holds it.
    BeforeCase,
    /// A call of a project macro whose body holds a jump token: it may
    /// leave the function, loop or switch unseen.
    HiddenJump,
}

impl NotDrawn {
    pub fn label(self) -> &'static str {
        match self {
            NotDrawn::Try => "?",
            NotDrawn::UnwrapOr => "unwrap_or",
            NotDrawn::Closure => "closure",
            NotDrawn::Ternary => "ternary",
            NotDrawn::TryBlock => "try",
            NotDrawn::ShortCircuit => "&&/||",
            NotDrawn::Goto => "goto",
            NotDrawn::Preprocessor => "#if",
            NotDrawn::Untraced => "call not traced",
            NotDrawn::NestedCase => "nested case",
            NotDrawn::BeforeCase => "code before the first case",
            NotDrawn::HiddenJump => "macro jump",
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
    /// A C++ method of a class declared in a function body, drawn inside
    /// that function: its members are the local class's.
    pub local_class: bool,
    pub syntax_errors: bool,
    /// The names written as whole statements (`GIVE_UP;`), nested flows'
    /// included: one a project macro with a jump is counted as one.
    pub words: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Branch {
        head: Option<String>,
        line: usize,
        arms: Vec<Arm>,
        /// No way past the last arm (a Rust `match`).
        exhaustive: bool,
        /// An unlabelled `break` in it, in no inner loop, leaves the branch
        /// (a C `switch`); `continue` passes through to the loop.
        breakable: bool,
        /// The way past the arms runs through branches not drawn (a
        /// directive group's others, in a first-branch chart): the one arm
        /// is drawn in line, unless it cannot fall through and a step
        /// follows, which that way then reaches.
        unseen: bool,
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
        /// The body runs before the header's test (`do … while`): the first
        /// edge enters the body, `pre` and the header follow it.
        test_after: bool,
        /// Run after the body and before the header on every iteration (the
        /// update of a C `for`); the body's fall-off and `continue` enter it.
        post: Vec<Step>,
    },
    Call(MapCall),
    Exit {
        text: String,
        line: usize,
        kind: ExitKind,
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
    /// The open ends of `body` join the next arm's body instead of leaving
    /// the branch (a C `case` without `break`); the last arm's leave it.
    pub falls: bool,
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
    /// A path call's callee as written, to its arguments: `text` is clipped.
    pub path: Option<String>,
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
