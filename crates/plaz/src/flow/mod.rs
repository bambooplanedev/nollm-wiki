//! The flow stage: each function body as a decision flow, walked by one
//! walker that asks a language's `Hooks` what each node is.

mod python;
mod rust;

pub(crate) use python::{module_code, module_imports};

use crate::model::{Arm, Comment, ExitKind, File, Flow, MapCall, NotDrawn, Step, Target};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use wiki_code::index::Family;
use wiki_code::model::{CallVia, CodeItemKind};
use wiki_code::support::{clip, one_line};
use wiki_code::tree_sitter::Node;
use wiki_code::{Analyzed, Source};

/// What one node is to the flow.
pub(crate) enum FlowNode<'a> {
    /// Not drawn itself; its children are walked, then its own calls.
    Plain,
    Branch {
        head: Option<String>,
        arms: Vec<ArmNode<'a>>,
        exhaustive: bool,
    },
    /// The children outside `body`, `each` and `orelse` are walked before
    /// the loop; `each`, run before every iteration, into its `pre`;
    /// `orelse` (Python's loop `else`) runs when the loop ends unbroken.
    Loop {
        header: String,
        body: Node<'a>,
        each: Option<Node<'a>>,
        label: Option<String>,
        endless: bool,
        hidden_exit: bool,
        orelse: Option<Node<'a>>,
    },
    /// A `finally` clause: its children are the body.
    Finally,
    Exit(ExitKind),
    #[expect(dead_code, reason = "no language draws an assignment yet")]
    Assign {
        target: Node<'a>,
        value: Node<'a>,
    },
    /// `body` is the node whose children form the nested flow and whose
    /// bindings are the nested flow's own; `None` names it `closure L<n>`.
    Nested {
        name: Option<String>,
        body: Node<'a>,
    },
    /// Counted, then walked as `Plain`.
    NotDrawn(NotDrawn),
    /// Belongs to no step of this flow.
    #[expect(dead_code, reason = "Rust and Python draw every node")]
    Skip,
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
    /// Walked as the arm's `after`.
    pub after: Option<Node<'a>>,
}

#[derive(Default)]
pub(crate) struct Bindings {
    pub names: BTreeSet<String>,
    pub types: BTreeMap<String, String>,
    /// See `Flow::rebound`.
    pub rebound: BTreeSet<String>,
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
            rebound: BTreeSet::new(),
        }
    }
}

pub(crate) struct Hooks {
    pub flow_node: for<'a> fn(Node<'a>, &str) -> FlowNode<'a>,
    pub bindings: fn(Node, &str) -> Bindings,
}

pub(crate) fn hooks(family: Family) -> Option<&'static Hooks> {
    match family {
        Family::Rust => Some(&rust::HOOKS),
        Family::Python => Some(&python::HOOKS),
        Family::Cpp | Family::Go | Family::Shell | Family::Ecma => None,
    }
}

/// Fill `file.flows` for every function item that is not a test, not nested
/// and has a node; count the tests in `file.test_fns`.
pub(crate) fn attach(file: &mut File, analyzed: &Analyzed) {
    let Some(hooks) = hooks(file.family) else {
        return;
    };
    for (i, (it, def)) in file.code.items.iter().zip(&analyzed.defs).enumerate() {
        let is_fn = matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method);
        let Some(def) = def.as_ref().filter(|_| is_fn && !it.nested) else {
            continue;
        };
        if it.test {
            file.test_fns += 1;
            continue;
        }
        let source = &analyzed.sources[def.source];
        if let Some(node) = source.node(def) {
            let w = Walker {
                source,
                hooks,
                comments: comments(source, node),
            };
            file.flows.insert(i, w.flow(node));
        }
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
}

fn line(n: Node) -> usize {
    n.start_position().row + 1
}

impl<'s> Walker<'s> {
    fn text(&self, n: Node) -> &'s str {
        &self.source.text()[n.byte_range()]
    }

    /// The flow of `root`'s children, with `root`'s bindings.
    fn flow(&self, root: Node) -> Flow {
        let b = (self.hooks.bindings)(root, self.source.text());
        let mut flow = Flow {
            locals: b.names,
            types: b.types,
            rebound: b.rebound,
            syntax_errors: root.has_error(),
            ..Flow::default()
        };
        let mut steps = Vec::new();
        self.children(root, &mut flow, &mut steps);
        flow.steps = steps;
        flow
    }

    fn children(&self, n: Node, flow: &mut Flow, out: &mut Vec<Step>) {
        for c in n.children(&mut n.walk()) {
            self.walk(c, flow, out);
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

    fn walk(&self, n: Node, flow: &mut Flow, out: &mut Vec<Step>) {
        match (self.hooks.flow_node)(n, self.source.text()) {
            FlowNode::Skip => {}
            FlowNode::Plain => {
                self.children(n, flow, out);
                self.calls(n, out);
            }
            FlowNode::NotDrawn(k) => {
                *flow.not_drawn.entry(k).or_default() += 1;
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
            FlowNode::Assign { target, value } => {
                self.walk(value, flow, out);
                out.push(Step::Assign {
                    target: one_line(self.text(target)),
                    value: one_line(self.text(value)),
                    line: line(n),
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
            FlowNode::Nested { name, body } => out.push(Step::Nested {
                name: name.unwrap_or_else(|| format!("closure L{}", line(n))),
                line: line(n),
                flow: self.flow(body),
            }),
            FlowNode::Loop {
                header,
                body,
                each,
                label,
                endless,
                hidden_exit,
                orelse,
            } => {
                let except: Vec<Node> = each.into_iter().chain([body]).chain(orelse).collect();
                let rest = self.around(n, &except, flow, out);
                let (mut pre, mut steps, mut after) = (Vec::new(), Vec::new(), Vec::new());
                if let Some(e) = each {
                    self.walk(e, flow, &mut pre);
                }
                self.walk(body, flow, &mut steps);
                if let Some(e) = orelse {
                    self.walk(e, flow, &mut after);
                }
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
                });
                for c in rest {
                    self.walk(c, flow, out);
                }
            }
            FlowNode::Branch {
                head,
                arms,
                exhaustive,
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
                        let (mut pre, mut body, mut late) = (Vec::new(), Vec::new(), Vec::new());
                        if let Some(c) = a.cond_node {
                            self.walk(c, flow, &mut pre);
                        }
                        for b in a.body {
                            self.walk(b, flow, &mut body);
                        }
                        if let Some(l) = a.after {
                            self.walk(l, flow, &mut late);
                        }
                        Arm {
                            cond: a.cond,
                            line: line(a.at),
                            comment,
                            pre,
                            body,
                            after: late,
                        }
                    })
                    .collect();
                out.push(Step::Branch {
                    head,
                    line: line(n),
                    arms,
                    exhaustive,
                });
                for c in rest {
                    self.walk(c, flow, out);
                }
            }
        }
    }

    fn calls(&self, n: Node, out: &mut Vec<Step>) {
        let text = self.source.text();
        for c in self.source.calls_at(n) {
            let lit = &text[c.start_byte..c.end_byte];
            let root = matches!(c.site.via, CallVia::Path(_))
                .then(|| lit.split("::").next().unwrap_or("").trim().to_string());
            out.push(Step::Call(MapCall {
                text: clip(lit, 80),
                site: c.site,
                root,
                target: Target::Hidden,
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

/// A copy of one enclosing flow's names, for [`Flow::each_call_mut`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowScope {
    pub locals: BTreeSet<String>,
    pub types: BTreeMap<String, String>,
    pub rebound: BTreeSet<String>,
}

impl Flow {
    /// Every call once, in source order, nested flows included, with the
    /// chain of flows enclosing it, outermost first.
    pub fn each_call<'a>(&'a self, f: &mut impl FnMut(&[&'a Flow], &'a MapCall)) {
        fn walk<'a>(
            steps: &'a [Step],
            chain: &mut Vec<&'a Flow>,
            f: &mut impl FnMut(&[&'a Flow], &'a MapCall),
        ) {
            for s in steps {
                match s {
                    Step::Call(c) => f(chain, c),
                    Step::Branch { arms, .. } => {
                        for a in arms {
                            walk(&a.pre, chain, f);
                            walk(&a.body, chain, f);
                        }
                        for a in arms {
                            walk(&a.after, chain, f);
                        }
                    }
                    Step::Loop {
                        pre, body, orelse, ..
                    } => {
                        walk(pre, chain, f);
                        walk(body, chain, f);
                        walk(orelse, chain, f);
                    }
                    Step::Finally { body, .. } => walk(body, chain, f),
                    Step::Nested { flow, .. } => {
                        chain.push(flow);
                        walk(&flow.steps, chain, f);
                        chain.pop();
                    }
                    Step::Exit { .. } | Step::Assign { .. } => {}
                }
            }
        }
        walk(&self.steps, &mut vec![self], f);
    }

    /// [`Flow::each_call`] with each call mutable and the chain as copies.
    pub fn each_call_mut(&mut self, f: &mut impl FnMut(&[FlowScope], &mut MapCall)) {
        fn walk(
            steps: &mut [Step],
            chain: &mut Vec<FlowScope>,
            f: &mut impl FnMut(&[FlowScope], &mut MapCall),
        ) {
            for s in steps {
                match s {
                    Step::Call(c) => f(chain, c),
                    Step::Branch { arms, .. } => {
                        for a in arms.iter_mut() {
                            walk(&mut a.pre, chain, f);
                            walk(&mut a.body, chain, f);
                        }
                        for a in arms {
                            walk(&mut a.after, chain, f);
                        }
                    }
                    Step::Loop {
                        pre, body, orelse, ..
                    } => {
                        walk(pre, chain, f);
                        walk(body, chain, f);
                        walk(orelse, chain, f);
                    }
                    Step::Finally { body, .. } => walk(body, chain, f),
                    Step::Nested { flow, .. } => scoped(flow, chain, f),
                    Step::Exit { .. } | Step::Assign { .. } => {}
                }
            }
        }
        fn scoped(
            flow: &mut Flow,
            chain: &mut Vec<FlowScope>,
            f: &mut impl FnMut(&[FlowScope], &mut MapCall),
        ) {
            chain.push(FlowScope {
                locals: flow.locals.clone(),
                types: flow.types.clone(),
                rebound: flow.rebound.clone(),
            });
            walk(&mut flow.steps, chain, f);
            chain.pop();
        }
        scoped(self, &mut Vec::new(), f);
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::model::{File, Step};

    pub fn file(path: &str, src: &str) -> File {
        crate::source::project([(path.to_string(), src.to_string())])
            .files
            .remove(path)
            .unwrap()
    }

    fn flow<'f>(f: &'f File, name: &str) -> &'f crate::model::Flow {
        let i = f.code.items.iter().position(|it| it.name == name).unwrap();
        &f.flows[&i]
    }

    /// One line per step, two spaces per depth.
    pub fn flow_lines(path: &str, src: &str, name: &str) -> Vec<String> {
        fn lines(steps: &[Step], depth: usize, out: &mut Vec<String>) {
            let pad = "  ".repeat(depth);
            for s in steps {
                match s {
                    Step::Branch {
                        head, line, arms, ..
                    } => {
                        let head = head.as_ref().map_or(String::new(), |h| format!(" {h}"));
                        out.push(format!("{pad}branch{head} @{line}"));
                        for a in arms {
                            let mut pre = Vec::new();
                            lines(&a.pre, depth + 1, &mut pre);
                            out.extend(
                                pre.into_iter()
                                    .map(|l| format!("{pad}  pre {}", &l[pad.len() + 2..])),
                            );
                            let cond = a.cond.as_deref().unwrap_or("else");
                            let note = a.comment.as_ref().map_or(String::new(), |c| {
                                format!(" # {}", c.text.replace('\n', "\\n"))
                            });
                            out.push(format!("{pad}  arm {cond} @{}{note}", a.line));
                            lines(&a.body, depth + 2, out);
                        }
                        for a in arms.iter().filter(|a| !a.after.is_empty()) {
                            out.push(format!(
                                "{pad}  after {}",
                                a.cond.as_deref().unwrap_or("else")
                            ));
                            lines(&a.after, depth + 2, out);
                        }
                    }
                    Step::Finally { line, body } => {
                        out.push(format!("{pad}finally @{line}"));
                        lines(body, depth + 1, out);
                    }
                    Step::Loop {
                        header,
                        line,
                        pre,
                        body,
                        orelse,
                        ..
                    } => {
                        out.push(format!("{pad}loop {header} @{line}"));
                        let mut p = Vec::new();
                        lines(pre, depth + 1, &mut p);
                        out.extend(
                            p.into_iter()
                                .map(|l| format!("{pad}  pre {}", &l[pad.len() + 2..])),
                        );
                        lines(body, depth + 1, out);
                        if !orelse.is_empty() {
                            out.push(format!("{pad}  else"));
                            lines(orelse, depth + 2, out);
                        }
                    }
                    Step::Call(c) => out.push(format!("{pad}call {} @{}", c.text, c.site.line)),
                    Step::Exit { text, line, .. } => out.push(format!("{pad}exit {text} @{line}")),
                    Step::Assign {
                        target,
                        value,
                        line,
                    } => out.push(format!("{pad}assign {target} = {value} @{line}")),
                    Step::Nested { name, line, flow } => {
                        out.push(format!("{pad}nested {name} @{line}"));
                        lines(&flow.steps, depth + 1, out);
                    }
                }
            }
        }
        let f = file(path, src);
        let mut out = Vec::new();
        lines(&flow(&f, name).steps, 0, &mut out);
        out
    }

    /// The function's not-drawn counts by label, sorted by label.
    pub fn not_drawn(path: &str, src: &str, name: &str) -> Vec<(String, usize)> {
        let f = file(path, src);
        let mut v: Vec<_> = flow(&f, name)
            .not_drawn
            .iter()
            .map(|(k, n)| (k.label().to_string(), *n))
            .collect();
        v.sort();
        v
    }
}
