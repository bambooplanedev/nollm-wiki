//! The flow stage: each function body as a decision flow, walked by one
//! walker that asks a language's `Hooks` what each node is.

mod calls;
mod cpp;
mod python;
mod rust;

pub use calls::FlowScope;

pub(crate) use cpp::noreturn_word;
pub(crate) use cpp::{fields as cpp_fields, macros as cpp_macros, module_code as cpp_module_code};
pub(crate) use python::{module_code, module_imports};

use crate::model::{Arm, Comment, ExitKind, File, Flow, MapCall, NotDrawn, Step, Target};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use wiki_code::index::Family;
use wiki_code::model::{CallSite, CallVia, CodeItemKind};
use wiki_code::support::{clip, one_line};
use wiki_code::tree_sitter::Node;
use wiki_code::{Analyzed, Def, Source};

/// What one node is to the flow.
pub(crate) enum FlowNode<'a> {
    /// Not drawn itself; its children are walked, then its own calls.
    Plain,
    Branch {
        head: Option<String>,
        arms: Vec<ArmNode<'a>>,
        exhaustive: bool,
        breakable: bool,
    },
    /// The children outside `body`, `each` and `orelse` are walked before
    /// the loop; `each`, run before every iteration, into its `pre`;
    /// `post`, run after the body, into its `post`;
    /// `orelse` (Python's loop `else`) runs when the loop ends unbroken.
    Loop {
        header: String,
        body: Node<'a>,
        each: Option<Node<'a>>,
        label: Option<String>,
        endless: bool,
        hidden_exit: bool,
        orelse: Option<Node<'a>>,
        test_after: bool,
        /// See `Step::Loop::once`.
        once: bool,
        post: Option<Node<'a>>,
    },
    /// A `finally` clause: its children are the body.
    Finally,
    Exit(ExitKind),
    /// `body` is the node whose children form the nested flow and whose
    /// bindings are the nested flow's own; `None` names it `closure L<n>`.
    Nested {
        name: Option<String>,
        body: Node<'a>,
    },
    /// Counted, then walked as `Plain`.
    NotDrawn(NotDrawn),
    /// A name written as a whole statement (a C macro): kept in
    /// `Flow::words`, then walked as `Plain`.
    Word(String),
}

pub(crate) struct ArmNode<'a> {
    /// `None` is the else arm.
    pub cond: Option<String>,
    /// Walked into the arm's `pre`.
    pub cond_node: Option<Node<'a>>,
    /// Where the arm starts: its line and the anchor of its comment.
    pub at: Node<'a>,
    /// Walked in order as the arm's body.
    pub body: Vec<Node<'a>>,
    pub falls: bool,
    /// Walked as the arm's `after`.
    pub after: Option<Node<'a>>,
}

#[derive(Default)]
pub(crate) struct Bindings {
    pub names: BTreeSet<String>,
    pub types: BTreeMap<String, String>,
    /// See `Flow::params`.
    pub params: BTreeSet<String>,
    /// See `Flow::outside`.
    pub outside: BTreeSet<String>,
    /// See `Flow::rebound`.
    pub rebound: BTreeSet<String>,
    /// See `Flow::local_class`.
    pub local_class: bool,
    /// See `Flow::inner`.
    pub inner: BTreeSet<String>,
}

impl Bindings {
    /// The conservative join of every binding a flow makes: a name keeps a
    /// type only when each of its bindings gives it that same type.
    pub fn join(binds: impl IntoIterator<Item = (String, Option<String>)>) -> Bindings {
        let mut seen: BTreeMap<String, Option<String>> = BTreeMap::new();
        for (name, ty) in binds {
            seen.entry(name)
                .and_modify(|t| {
                    if *t != ty {
                        *t = None;
                    }
                })
                .or_insert(ty);
        }
        Bindings {
            types: seen
                .iter()
                .filter_map(|(n, t)| Some((n.clone(), t.clone()?)))
                .collect(),
            names: seen.into_keys().collect(),
            ..Bindings::default()
        }
    }
}

pub(crate) struct Hooks {
    pub flow_node: for<'a> fn(Node<'a>, &str) -> FlowNode<'a>,
    pub bindings: fn(Node, &str) -> Bindings,
    /// A call site that is no call to chart (a C++ cast).
    pub skips_call: fn(&CallSite) -> bool,
    /// Is the call node a whole statement on the path (`MapCall::ends`)?
    pub ends_path: fn(Node, &str) -> bool,
}

/// Is the call node `n` the whole of an expression statement?
fn whole_statement(n: Node) -> bool {
    n.parent()
        .is_some_and(|p| p.kind() == "expression_statement" && p.named_child_count() == 1)
}

pub(crate) fn hooks(family: Family) -> Option<&'static Hooks> {
    match family {
        Family::Rust => Some(&rust::HOOKS),
        Family::Python => Some(&python::HOOKS),
        Family::Cpp => Some(&cpp::HOOKS),
        Family::Go | Family::Shell | Family::Ecma => None,
    }
}

/// Fill `file.flows` for every function item that is not a test, not nested
/// and has a node that is a function definition, and `file.declarations`
/// for those whose node is a declaration; count the tests in
/// `file.test_fns`.
///
/// With two trees (C/C++, `sources` being `[first-branch copy, main]`), a
/// function of the copy is walked in the main tree when its range there
/// is a whole function definition; otherwise in the copy, with the
/// directive groups in its range counted. A main-tree item overlapping a
/// copy function is that function read from another branch: marked
/// `nested`, so it has no section, no anchor and is no call target.
pub(crate) fn attach(file: &mut File, analyzed: &Analyzed) {
    let Some(hooks) = hooks(file.family) else {
        return;
    };
    let main = analyzed.sources.len().saturating_sub(1);
    let fn_defs: Vec<(usize, &Def)> = (file.code.items.iter().zip(&analyzed.defs))
        .enumerate()
        .filter(|(_, (it, _))| matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method))
        .filter_map(|(i, (it, d))| Some((i, d.as_ref().filter(|_| !it.nested)?)))
        .collect();
    for &(i, def) in &fn_defs {
        let it = &file.code.items[i];
        let twice = main > 0
            && def.source == main
            && (fn_defs.iter()).any(|(_, c)| {
                c.source < main && c.start_byte < def.end_byte && def.start_byte < c.end_byte
            });
        let mut source = &analyzed.sources[def.source];
        if twice {
            file.code.items[i].nested = true;
            continue;
        }
        let Some(mut node) = source.node(def) else {
            continue;
        };
        if let Some(b) = (file.family == Family::Cpp)
            .then(|| cpp::bodiless(node))
            .flatten()
        {
            if !it.test {
                file.cpp.declarations.insert(i, b);
            }
            continue;
        }
        if it.test {
            file.test_fns += 1;
            continue;
        }
        let mut groups = Vec::new();
        if def.source < main {
            let whole = &analyzed.sources[main];
            match whole
                .node(def)
                .filter(|m| m.kind() == "function_definition" && !m.has_error())
            {
                Some(m) => (source, node) = (whole, m),
                // The directives are blank in the first-branch copy: read
                // them from the file's text, which the offsets also index.
                None => groups = cpp::directive_groups(whole.text(), def.start_byte..def.end_byte),
            }
        }
        let w = Walker {
            source,
            hooks,
            comments: comments(source, node),
            groups,
            drawn: RefCell::new(BTreeSet::new()),
        };
        let mut flow = w.flow(node);
        let hidden = w.groups.len() - w.drawn.borrow().len();
        let early = match file.family {
            Family::Cpp => cpp::before_case(node, source.text()),
            Family::Rust | Family::Python | Family::Go | Family::Shell | Family::Ecma => 0,
        };
        for (k, n) in [
            (NotDrawn::Preprocessor, hidden),
            (NotDrawn::BeforeCase, early),
        ] {
            if n > 0 {
                *flow.not_drawn.entry(k).or_default() += n;
            }
        }
        file.flows.insert(i, flow);
    }
}

struct Note {
    /// 0-based rows, end exclusive.
    rows: Range<usize>,
    /// Nothing but whitespace before it on its first line: a trailing
    /// comment belongs to the code before it.
    own_line: bool,
    start_byte: usize,
    text: String,
}

/// Every comment under a function, in source order.
fn comments(source: &Source, root: Node) -> Vec<Note> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if source.is_comment(n) {
            let before = &source.text()[..n.start_byte()];
            out.push(Note {
                rows: n.start_position().row..n.end_position().row + 1,
                own_line: before[before.rfind('\n').map_or(0, |i| i + 1)..]
                    .trim()
                    .is_empty(),
                start_byte: n.start_byte(),
                text: source.comment_text(n),
            });
        }
        stack.extend(n.children(&mut n.walk()));
    }
    out.sort_by_key(|c| c.rows.start);
    out
}

struct Walker<'s> {
    source: &'s Source,
    hooks: &'static Hooks,
    comments: Vec<Note>,
    /// Charting from a C/C++ first-branch tree: the directive groups in
    /// the function's range, and the indices of those drawn as branches.
    groups: Vec<cpp::Group>,
    drawn: RefCell<BTreeSet<usize>>,
}

fn text<'t>(n: Node, src: &'t str) -> &'t str {
    &src[n.byte_range()]
}

fn line(n: Node) -> usize {
    n.start_position().row + 1
}

impl<'s> Walker<'s> {
    fn text(&self, n: Node) -> &'s str {
        text(n, self.source.text())
    }

    /// The flow of `root`'s children, with `root`'s bindings.
    fn flow(&self, root: Node) -> Flow {
        let b = (self.hooks.bindings)(root, self.source.text());
        let mut flow = Flow {
            locals: b.names,
            types: b.types,
            params: b.params,
            outside: b.outside,
            rebound: b.rebound,
            local_class: b.local_class,
            inner: b.inner,
            syntax_errors: root.has_error(),
            ..Flow::default()
        };
        let mut steps = Vec::new();
        self.children(root, &mut flow, &mut steps);
        flow.steps = steps;
        flow
    }

    fn children(&self, n: Node, flow: &mut Flow, out: &mut Vec<Step>) {
        let kids: Vec<Node> = n.children(&mut n.walk()).collect();
        self.run(&kids, &n.byte_range(), flow, out);
    }

    /// Walks `kids` in order. A run of them in the first branch of a
    /// directive group opening in `within`, when it has a step, is the arm
    /// of that group's branch: if the group has no other branch, the way
    /// past it skips the run; else the group stays counted and the way
    /// past runs through branches not drawn (`Step::Branch::unseen`).
    fn run(&self, kids: &[Node], within: &Range<usize>, flow: &mut Flow, out: &mut Vec<Step>) {
        let mut kids = kids.iter().copied().peekable();
        while let Some(c) = kids.next() {
            let Some((i, g)) = cpp::holding(&self.groups, c, within) else {
                self.walk(c, flow, out);
                continue;
            };
            let mut body = vec![c];
            body.extend(std::iter::from_fn(|| {
                kids.next_if(|k| k.end_byte() <= g.body.end)
            }));
            let mut steps = Vec::new();
            self.run(&body, &g.body, flow, &mut steps);
            // A group holding no step (a `#define`) stays counted.
            if steps.is_empty() {
                continue;
            }
            if g.only {
                self.drawn.borrow_mut().insert(i);
            }
            out.push(Step::Branch {
                head: None,
                line: g.line,
                arms: vec![Arm {
                    cond: Some(g.cond.clone()),
                    line: g.line,
                    comment: None,
                    pre: Vec::new(),
                    body: steps,
                    falls: false,
                    after: Vec::new(),
                }],
                exhaustive: false,
                breakable: false,
                unseen: !g.only,
            });
        }
    }

    /// Walks `n`'s children that overlap none of `inner` and start before
    /// its end; returns those after it, which run after `n`'s own step.
    fn around<'t>(
        &self,
        n: Node<'t>,
        inner: &[Node],
        flow: &mut Flow,
        out: &mut Vec<Step>,
    ) -> Vec<Node<'t>> {
        let end = inner.iter().map(Node::end_byte).max().unwrap_or(usize::MAX);
        let overlaps = |c: Node| {
            inner
                .iter()
                .any(|e| c.start_byte() < e.end_byte() && e.start_byte() < c.end_byte())
        };
        let mut after = Vec::new();
        for c in n.children(&mut n.walk()).filter(|&c| !overlaps(c)) {
            if c.start_byte() >= end {
                after.push(c);
            } else {
                self.walk(c, flow, out);
            }
        }
        after
    }

    /// The steps of `nodes`, walked in order as `run` walks a sequence:
    /// the groups that open among them (after `from`, else after the
    /// first) recognised.
    fn sub<'t>(
        &self,
        from: Option<usize>,
        nodes: impl IntoIterator<Item = Node<'t>>,
        flow: &mut Flow,
    ) -> Vec<Step> {
        let nodes: Vec<Node> = nodes.into_iter().collect();
        let start = from.or_else(|| nodes.first().map(Node::start_byte));
        let within = start.unwrap_or(0)..nodes.last().map_or(0, Node::end_byte);
        let mut out = Vec::new();
        self.run(&nodes, &within, flow, &mut out);
        out
    }

    fn walk(&self, n: Node, flow: &mut Flow, out: &mut Vec<Step>) {
        match (self.hooks.flow_node)(n, self.source.text()) {
            FlowNode::Plain => {
                self.children(n, flow, out);
                self.calls(n, out);
            }
            FlowNode::NotDrawn(k) => {
                *flow.not_drawn.entry(k).or_default() += 1;
                self.children(n, flow, out);
                self.calls(n, out);
            }
            FlowNode::Word(w) => {
                flow.words.push(w);
                self.children(n, flow, out);
                self.calls(n, out);
            }
            FlowNode::Exit(kind) => {
                self.children(n, flow, out);
                self.calls(n, out);
                out.push(Step::Exit {
                    text: one_line(self.text(n)),
                    line: line(n),
                    kind,
                });
            }
            FlowNode::Finally => {
                let mut body = Vec::new();
                self.children(n, flow, &mut body);
                out.push(Step::Finally {
                    line: line(n),
                    body,
                });
            }
            FlowNode::Nested { name, body } => {
                let mut nested = self.flow(body);
                flow.words.append(&mut nested.words);
                out.push(Step::Nested {
                    name: name.unwrap_or_else(|| format!("closure L{}", line(n))),
                    line: line(n),
                    flow: nested,
                });
            }
            FlowNode::Loop {
                header,
                body,
                each,
                label,
                endless,
                hidden_exit,
                orelse,
                test_after,
                once,
                post,
            } => {
                let except: Vec<Node> = each
                    .into_iter()
                    .chain([body])
                    .chain(post)
                    .chain(orelse)
                    .collect();
                let rest = self.around(n, &except, flow, out);
                let pre = self.sub(None, each, flow);
                let steps = self.sub(None, [body], flow);
                let post = self.sub(None, post, flow);
                let after = self.sub(None, orelse, flow);
                out.push(Step::Loop {
                    header,
                    line: line(n),
                    comment: self.comment(n, 0),
                    pre,
                    body: steps,
                    label,
                    endless,
                    hidden_exit,
                    orelse: after,
                    test_after,
                    once,
                    post,
                });
                out.extend(self.sub(None, rest, flow));
            }
            FlowNode::Branch {
                head,
                arms,
                exhaustive,
                breakable,
            } => {
                let inner: Vec<Node> = arms
                    .iter()
                    .flat_map(|a| a.cond_node.iter().chain(&a.body).chain(&a.after).copied())
                    .collect();
                let rest = self.around(n, &inner, flow, out);
                // A comment inside an arm's body is not the next arm's.
                let mut after = 0;
                let arms = arms
                    .into_iter()
                    .map(|a| {
                        let comment = self.comment(a.at, after);
                        after = a
                            .body
                            .last()
                            .or(a.cond_node.as_ref())
                            .map_or(after, Node::end_byte);
                        let pre = self.sub(None, a.cond_node, flow);
                        // A `case` arm's group may open right after its label.
                        let body = self.sub(Some(a.at.start_byte()), a.body, flow);
                        let late = self.sub(None, a.after, flow);
                        Arm {
                            cond: a.cond,
                            line: line(a.at),
                            comment,
                            pre,
                            body,
                            falls: a.falls,
                            after: late,
                        }
                    })
                    .collect();
                out.push(Step::Branch {
                    head,
                    line: line(n),
                    arms,
                    exhaustive,
                    breakable,
                    unseen: false,
                });
                out.extend(self.sub(None, rest, flow));
            }
        }
    }

    fn calls(&self, n: Node, out: &mut Vec<Step>) {
        let text = self.source.text();
        for c in self.source.calls_at(n) {
            if (self.hooks.skips_call)(&c.site) {
                continue;
            }
            let lit = &text[c.start_byte..c.end_byte];
            // The callee is the call node's own: a `(` may stand in its
            // template arguments (`K<decltype(f())>::g()`).
            let whole = |x: &Node| x.byte_range() == (c.start_byte..c.end_byte);
            let inner = n.descendant_for_byte_range(c.start_byte, c.end_byte);
            let node = std::iter::successors(inner, Node::parent)
                .take_while(whole)
                .find_map(|x| x.child_by_field_name("function"))
                .map(|f| &text[f.byte_range()]);
            // A chain's first `(` opens its receiver's call: a member call's
            // callee is its node's only.
            let callee = match c.site.via {
                CallVia::Path(_) => {
                    Some(node.unwrap_or_else(|| lit.split('(').next().unwrap_or("")))
                }
                CallVia::Member(None) => node,
                CallVia::Bare | CallVia::SelfRecv | CallVia::Member(Some(_)) => None,
            };
            out.push(Step::Call(MapCall {
                text: clip(lit, 80),
                site: c.site,
                callee: callee.map(str::to_string),
                target: Target::Hidden,
                ends: (self.hooks.ends_path)(n, text),
            }));
        }
    }

    /// The run of whole-line comments on the rows directly above `at`, then
    /// a comment starting on `at`'s row; none starting before byte `after`.
    fn comment(&self, at: Node, after: usize) -> Option<Comment> {
        let row = at.start_position().row;
        let mut all = Vec::new();
        let mut next = row;
        while let Some(c) = self
            .comments
            .iter()
            .rev()
            .find(|c| c.rows.end == next && c.own_line && c.start_byte >= after)
        {
            all.push(c);
            next = c.rows.start;
        }
        all.reverse();
        all.extend(
            self.comments
                .iter()
                .find(|c| c.rows.start == row && c.start_byte >= after),
        );
        let (first, last) = (all.first()?, all.last()?);
        Some(Comment {
            text: all
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            start_line: first.rows.start + 1,
            end_line: last.rows.end,
        })
    }
}

#[cfg(test)]
pub(crate) mod testutil;
