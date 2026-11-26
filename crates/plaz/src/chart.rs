//! The decision flowchart of one function and its coverage line.

use crate::model::{Arm, Comment, ExitKind, Flow, FnId, MapCall, NotDrawn, Step, Target};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Range;
use wiki_code::model::CallVia;

/// `s` as the text of a double-quoted Mermaid label: every char that could
/// end the label, the node or the fence becomes an entity code, a line break
/// `<br/>`.
pub fn mermaid_label(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '#' => out.push_str("#35;"),
            '"' => out.push_str("#quot;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            '&' => out.push_str("#amp;"),
            '|' => out.push_str("#124;"),
            '{' => out.push_str("#123;"),
            '}' => out.push_str("#125;"),
            '`' => out.push_str("#96;"),
            '\\' => out.push_str("#92;"),
            ':' => out.push_str("#58;"),
            '$' => out.push_str("#36;"),
            '\n' => out.push_str("<br/>"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// The page id `to` as a path from the directory of the page id `from`.
pub(crate) fn relative(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').collect();
    let from = &from[..from.len() - 1];
    let to: Vec<&str> = to.split('/').collect();
    let common = from
        .iter()
        .zip(&to[..to.len() - 1])
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    parts.join("/")
}

/// Open ends of a path: a node and the label of the edge that leaves it.
type Ends = Vec<(usize, Option<&'static str>)>;

/// A loop around the step being drawn.
struct Scope {
    label: Option<String>,
    /// Where a `continue` goes; `None` when the loop's `post` or a test
    /// after the body is drawn only after the body, which collects them.
    again: Option<usize>,
    /// The ends its `break`s leave open.
    breaks: Ends,
    continues: Ends,
}

struct Chart<'a> {
    path: &'a str,
    href: &'a dyn Fn(&FnId) -> String,
    lines: Vec<String>,
    clicks: Vec<String>,
    next: usize,
    edges: usize,
    /// The loops around the step being drawn, innermost last.
    loops: Vec<Scope>,
    /// The breakable branches around the step being drawn, innermost last:
    /// the number of loops around the branch, and the ends its `break`s
    /// leave open.
    branches: Vec<(usize, Ends)>,
    /// `closed` of each arm list asked already, by its address: shared
    /// with the scratch charts, so each list is drawn once to tell.
    closed: &'a RefCell<BTreeMap<*const Arm, bool>>,
    /// The node of each call drawn and the subgraph of each nested flow,
    /// by their bytes: the ends of the dotted edges between them.
    placed: BTreeMap<(usize, usize), usize>,
    /// The calls drawn as no node (`undrawn`).
    undrawn: &'a BTreeMap<(usize, usize), NotDrawn>,
    /// The node drawn last, when it holds a run of calls a next call of
    /// its form on the straight path joins.
    run: Option<Run>,
}

/// A node holding consecutive calls of one form that links nowhere.
struct Run {
    id: usize,
    /// Its line in `Chart::lines`.
    line: usize,
    shape: (&'static str, &'static str),
    texts: Vec<String>,
}

/// The most calls one node holds.
const RUN: usize = 8;

/// The line of node `id` of `shape` labelled `text`.
fn node_line(id: usize, (open, close): (&str, &str), text: &str) -> String {
    format!("    n{id}{open}\"{}\"{close}", mermaid_label(text))
}

impl Chart<'_> {
    fn id(&mut self) -> usize {
        self.next += 1;
        self.next - 1
    }

    /// A node of `shape` (its opening and closing delimiters) labelled
    /// `text`, with an edge from each of `from`.
    fn node(&mut self, shape: (&str, &str), text: &str, from: &[(usize, Option<&str>)]) -> usize {
        let id = self.id();
        self.lines.push(node_line(id, shape, text));
        self.edges_to(from, id);
        id
    }

    fn edges_to(&mut self, from: &[(usize, Option<&str>)], to: usize) {
        self.run = None;
        for (f, label) in from {
            self.edges += 1;
            self.lines.push(match label {
                Some(l) => format!("    n{f} -->|{l}| n{to}"),
                None => format!("    n{f} --> n{to}"),
            });
        }
    }

    /// `c` as a note hanging off node `at`; past 6 lines, where it is.
    fn note(&mut self, c: Option<&Comment>, at: usize) {
        let Some(c) = c else { return };
        let lines = c.end_line - c.start_line + 1;
        let text = if lines > 6 {
            format!(
                "comment: {lines} lines at {}:{}-{}",
                self.path, c.start_line, c.end_line
            )
        } else {
            c.text.clone()
        };
        if text.trim().is_empty() {
            return;
        }
        let id = self.node(("[/", "\\]"), &text, &[]);
        self.edges += 1;
        self.lines.push(format!("    n{at} -.- n{id}"));
    }

    /// The start node, the steps, and one `end` node joining every path
    /// that runs off the end.
    fn flow(&mut self, name: &str, steps: &[Step]) {
        let start = self.node(("([", "])"), name, &[]);
        let loops = std::mem::take(&mut self.loops);
        let branches = std::mem::take(&mut self.branches);
        let (_, ends) = self.steps(steps, vec![(start, None)]);
        if !ends.is_empty() {
            self.node(("([", "])"), "end", &ends);
        }
        self.loops = loops;
        self.branches = branches;
    }

    /// `steps` as a chart of their own, beside the path; returns the
    /// subgraph's id.
    fn beside(&mut self, name: &str, steps: &[Step]) -> usize {
        let id = self.id();
        self.lines
            .push(format!("    subgraph n{id} [\"{}\"]", mermaid_label(name)));
        self.flow(name, steps);
        self.lines.push("    end".to_string());
        id
    }

    /// A dotted edge labelled `label` between two nodes or subgraphs
    /// placed, by their bytes.
    fn dotted(&mut self, from: &Range<usize>, label: &str, to: &Range<usize>) {
        let at = |r: &Range<usize>| self.placed.get(&(r.start, r.end)).copied();
        if let (Some(f), Some(t)) = (at(from), at(to)) {
            self.edges += 1;
            self.lines.push(format!("    n{f} -.->|{label}| n{t}"));
        }
    }

    /// The index in `loops` of the loop labelled `label`, or the innermost.
    fn target(&self, label: Option<&String>) -> Option<usize> {
        match label {
            Some(l) => self
                .loops
                .iter()
                .rposition(|lp| lp.label.as_ref() == Some(l)),
            None => self.loops.len().checked_sub(1),
        }
    }

    /// Draws `steps` after `ends`; returns the ends left open and the
    /// first node of the first step that is on the path: a nested flow's
    /// subgraph is beside it, not on it, as is a `finally` no path falls
    /// into.
    fn steps(&mut self, steps: &[Step], mut ends: Ends) -> (Option<usize>, Ends) {
        // The first step is a node of its own: what comes before may be
        // reached by edges that skip it (a loop's way back).
        self.run = None;
        let mut entry = None;
        for (i, s) in steps.iter().enumerate() {
            let nested = |s: &Step| matches!(s, Step::Nested { .. });
            let beside = nested(s) || (matches!(s, Step::Finally { .. }) && ends.is_empty());
            let first = self.next;
            let next = !steps[i + 1..].iter().all(nested);
            ends = self.step(s, ends, next);
            if entry.is_none() && !beside && self.next > first {
                entry = Some(first);
            }
        }
        (entry, ends)
    }

    /// `next`: a step of the same list follows `s`.
    fn step(&mut self, s: &Step, ends: Ends, next: bool) -> Ends {
        match s {
            Step::Call(c) if self.undrawn.contains_key(&(c.bytes.start, c.bytes.end)) => ends,
            Step::Call(c) => {
                let shape = match &c.target {
                    _ if c.local.is_some() => ("(", ")"),
                    Target::Resolved(_) => ("[", "]"),
                    Target::OneOf { .. } => ("[[", "]]:::oneof"),
                    Target::Outside => ("[/", "/]"),
                    Target::Hidden => (">", "]:::hidden"),
                };
                // Calls that link nowhere and are not an end of a dotted
                // edge or an exit join the run of their form before them
                // on the straight path, one per line.
                let folds = matches!(c.target, Target::Outside | Target::Hidden)
                    && c.local.is_none()
                    && c.takes.is_empty()
                    && !(c.ends && c.target != Target::Hidden);
                let key = (c.bytes.start, c.bytes.end);
                if let Some(r) = self.run.as_mut().filter(|r| {
                    folds && r.shape == shape && r.texts.len() < RUN && ends == [(r.id, None)]
                }) {
                    r.texts.push(c.text.clone());
                    let line = node_line(r.id, shape, &r.texts.join("\n"));
                    let (id, at) = (r.id, r.line);
                    self.lines[at] = line;
                    self.placed.insert(key, id);
                    return ends;
                }
                let line = self.lines.len();
                let id = self.node(shape, &c.text, &ends);
                self.placed.insert(key, id);
                if folds {
                    self.run = Some(Run {
                        id,
                        line,
                        shape,
                        texts: vec![c.text.clone()],
                    });
                }
                if let Target::Resolved(t) = &c.target {
                    let href = (self.href)(t);
                    self.clicks.push(format!("    click n{id} href \"{href}\""));
                }
                // A call that never returns is drawn as an exit after it;
                // one not resolved yet (hidden) ends nothing: resolution
                // clears `ends` for hidden calls, but charts of unresolved
                // flows (the flow unit tests) carry the walker's raw mark.
                if c.ends && c.target != Target::Hidden {
                    self.node(("([", "])"), &c.text, &[(id, None)]);
                    return Vec::new();
                }
                vec![(id, None)]
            }
            Step::Exit { text, kind, .. } => {
                match kind {
                    ExitKind::Return => {
                        self.node(("([", "])"), text, &ends);
                    }
                    // Outside any loop it is drawn, and ends there.
                    ExitKind::Break(label) => {
                        let id = self.node(("[", "]"), text, &ends);
                        let depth = self.loops.len();
                        match self.branches.last_mut() {
                            // An unlabelled `break` with no loop inside
                            // the branch leaves the branch.
                            Some((d, out)) if label.is_none() && *d == depth => {
                                out.push((id, None));
                            }
                            _ => {
                                if let Some(i) = self.target(label.as_ref()) {
                                    self.loops[i].breaks.push((id, None));
                                }
                            }
                        }
                    }
                    ExitKind::Continue(label) => {
                        let id = self.node(("[", "]"), text, &ends);
                        if let Some(i) = self.target(label.as_ref()) {
                            match self.loops[i].again {
                                Some(to) => self.edges_to(&[(id, None)], to),
                                None => self.loops[i].continues.push((id, None)),
                            }
                        }
                    }
                }
                Vec::new()
            }
            Step::Loop { .. } => self.looped(s, ends, next),
            Step::Branch {
                arms, unseen: true, ..
            } if !(next && self.closed(arms)) => {
                let ends = arms.iter().fold(ends, |e, a| self.steps(&a.body, e).1);
                // A branch not drawn still ends a run.
                self.run = None;
                ends
            }
            Step::Branch { .. } => self.branched(s, ends),
            Step::Nested { name, flow, at, .. } => {
                let id = self.beside(name, &flow.steps);
                self.placed.insert((at.start, at.end), id);
                ends
            }
            // The exit that passed through it goes on from its end, undrawn.
            Step::Finally { line, body } if ends.is_empty() => {
                self.beside(&format!("finally L{line}"), body);
                ends
            }
            Step::Finally { body, .. } => self.steps(body, ends).1,
        }
    }

    /// A loop: the header, the body, the `post`, and the ways out.
    fn looped(&mut self, s: &Step, ends: Ends, next: bool) -> Ends {
        let Step::Loop {
            header,
            comment,
            pre,
            body,
            label,
            endless,
            hidden_exit,
            orelse,
            test_after,
            once,
            post,
            ..
        } = s
        else {
            return ends;
        };
        let shape = if *endless { ("[", "]") } else { ("{", "}") };
        let scope = |again| Scope {
            label: label.clone(),
            again,
            breaks: Vec::new(),
            continues: Vec::new(),
        };
        let (d, breaks) = if *test_after {
            // The body runs first; its fall-off and every
            // `continue` go on to `post`, then the header.
            self.loops.push(scope(None));
            let (entry, back) = self.steps(body, ends);
            let lp = self.loops.pop().unwrap_or_else(|| scope(None));
            let (_, back) = self.steps(post, [back, lp.continues].concat());
            let (test, at) = self.steps(pre, back);
            let d = self.node(shape, header, &at);
            self.note(comment.as_ref(), d);
            // Each pass starts at the body's first node, else what runs
            // before the test.
            let again = entry.or(test).unwrap_or(d);
            if !once {
                self.edges_to(&[(d, Some("loop"))], again);
            }
            (d, lp.breaks)
        } else {
            // A header run before every iteration is where each
            // iteration starts.
            let (entry, at) = self.steps(pre, ends);
            let d = self.node(shape, header, &at);
            let again = entry.unwrap_or(d);
            self.note(comment.as_ref(), d);
            self.loops.push(scope((post.is_empty()).then_some(again)));
            let (_, back) = self.steps(body, vec![(d, Some("loop"))]);
            let lp = self.loops.pop().unwrap_or_else(|| scope(None));
            let (_, back) = self.steps(post, [back, lp.continues].concat());
            self.edges_to(&back, again);
            (d, lp.breaks)
        };
        let mut out = breaks;
        if !*endless {
            // A `break` skips the `else`.
            out.extend(self.steps(orelse, vec![(d, Some("done"))]).1);
        } else if !orelse.is_empty() {
            self.beside("else, never run", orelse);
        }
        if *endless && out.is_empty() && (next || *hidden_exit) {
            // A way out the flow cannot see: code right after
            // the loop would be dead without one, and a macro
            // with a `break` token may hold one.
            out.push((d, Some("exit not drawn")));
        }
        out
    }

    /// Can no arm of `arms` fall off its end? Drawn on a scratch chart
    /// with no loop around: a `break` or `continue` leaves too.
    fn closed(&self, arms: &[Arm]) -> bool {
        if let Some(&known) = self.closed.borrow().get(&arms.as_ptr()) {
            return known;
        }
        let mut scratch = Chart {
            path: self.path,
            href: self.href,
            lines: Vec::new(),
            clicks: Vec::new(),
            next: 1,
            edges: 0,
            loops: Vec::new(),
            branches: Vec::new(),
            closed: self.closed,
            placed: BTreeMap::new(),
            undrawn: self.undrawn,
            run: None,
        };
        let closed = (arms.iter()).all(|a| scratch.steps(&a.body, vec![(0, None)]).1.is_empty());
        self.closed.borrow_mut().insert(arms.as_ptr(), closed);
        closed
    }

    /// A branch: a diamond per arm, the open ends of every arm joined.
    fn branched(&mut self, s: &Step, ends: Ends) -> Ends {
        let Step::Branch {
            head,
            arms,
            exhaustive,
            breakable,
            unseen,
            ..
        } = s
        else {
            return ends;
        };
        let mut no = ends;
        if let Some(h) = head {
            no = vec![(self.node(("[", "]"), h, &no), None)];
        }
        if *breakable {
            self.branches.push((self.loops.len(), Vec::new()));
        }
        let mut out = Vec::new();
        // The open ends of a falling arm, to join the next body.
        let mut fell = Vec::new();
        for a in arms {
            let (_, at) = self.steps(&a.pre, no);
            no = if let Some(cond) = &a.cond {
                let d = self.node(("{", "}"), cond, &at);
                self.note(a.comment.as_ref(), d);
                let from = [vec![(d, Some("yes"))], fell].concat();
                let (_, ends) = self.steps(&a.body, from);
                let ends = self.steps(&a.after, ends).1;
                fell = Vec::new();
                if a.falls {
                    fell = ends;
                } else {
                    out.extend(ends);
                }
                vec![(d, Some("no"))]
            } else {
                // No diamond: the note hangs off the body's
                // first node, or the diamond that leads here.
                let from = [at.clone(), std::mem::take(&mut fell)].concat();
                let (entry, ends) = self.steps(&a.body, from);
                let ends = self.steps(&a.after, ends).1;
                if a.falls {
                    fell = ends;
                } else {
                    out.extend(ends);
                }
                if let Some(to) = entry.or_else(|| at.first().map(|e| e.0)) {
                    self.note(a.comment.as_ref(), to);
                }
                Vec::new()
            };
        }
        out.extend(fell);
        if !*exhaustive {
            let way = |(d, l)| (d, if *unseen { Some("exit not drawn") } else { l });
            out.extend(no.into_iter().map(way));
        }
        if *breakable {
            out.extend(self.branches.pop().map(|b| b.1).unwrap_or_default());
        }
        out
    }
}

/// The calls of `flow` drawn as no node of their own, by their bytes: a
/// chain step or a wrapper outside the project (or a call of a hidden
/// target whose receiver is such a chain step), never one that takes a
/// nested flow or calls one.
fn undrawn(flow: &Flow) -> BTreeMap<(usize, usize), NotDrawn> {
    let mut all = Vec::new();
    flow.each_call(&mut |_, c| all.push(c));
    // The calls that start where `c` does and are longer or shorter.
    let starting = |c: &MapCall, longer: bool| -> Vec<&MapCall> {
        (all.iter().copied())
            .filter(|b| b.bytes.start == c.bytes.start && b.bytes != c.bytes)
            .filter(|b| (b.bytes.end > c.bytes.end) == longer)
            .collect()
    };
    let mut out = BTreeMap::new();
    for c in &all {
        if !c.takes.is_empty() || c.local.is_some() || c.ends {
            continue;
        }
        let (outside, hidden) = (c.target == Target::Outside, c.target == Target::Hidden);
        let kind = if c.wrapper && (outside || hidden) {
            NotDrawn::Wrapper
        } else if !starting(c, true).is_empty()
            && (outside
                || hidden
                    && matches!(c.site.via, CallVia::Member(None))
                    && starting(c, false)
                        .iter()
                        .any(|r| r.target == Target::Outside))
        {
            NotDrawn::ChainStep
        } else {
            continue;
        };
        out.insert((c.bytes.start, c.bytes.end), kind);
    }
    out
}

/// The fenced `mermaid` block of `flow` (only a notice when it is over 50,000
/// characters), named `name`, in the file at
/// `path`; `href` gives a resolved target's link from this page.
pub(crate) fn chart(name: &str, path: &str, flow: &Flow, href: &dyn Fn(&FnId) -> String) -> String {
    let mut c = Chart {
        path,
        href,
        lines: Vec::new(),
        clicks: Vec::new(),
        next: 0,
        edges: 0,
        loops: Vec::new(),
        branches: Vec::new(),
        closed: &RefCell::new(BTreeMap::new()),
        placed: BTreeMap::new(),
        undrawn: &undrawn(flow),
        run: None,
    };
    c.flow(name, &flow.steps);
    // A nested flow is drawn beside the path: dotted edges tie it to the
    // calls that take it and to those that call it by name.
    // None from a call inside the subgraph: a nested function calling or
    // passing itself.
    flow.each_call(&mut |_, call| {
        let inside = |at: &Range<usize>| at.start <= call.bytes.start && call.bytes.end <= at.end;
        for at in call.takes.iter().filter(|at| !inside(at)) {
            c.dotted(at, "passed to", &call.bytes);
        }
        if let Some(l) = call.local.as_ref().filter(|l| !inside(&l.at)) {
            c.dotted(&call.bytes, "calls", &l.at);
        }
    });
    let mut body = String::from("flowchart TD\n");
    for l in c.lines.iter().chain(&c.clicks) {
        body.push_str(l);
        body.push('\n');
    }
    body.push_str(
        "    classDef oneof stroke-dasharray: 5 5\n    classDef hidden stroke-dasharray: 2 2\n",
    );
    if body.chars().count() > 50_000 {
        return "This chart exceeds Mermaid's 50,000-character limit and is not drawn; the Calls list below is the complete view.\n".into();
    }
    let mut out = format!("```mermaid\n{body}```\n");
    if c.edges > 500 {
        out.push_str("\nThis chart exceeds the 500-edge limit of Mermaid.js renderers other than Obsidian's; the Calls list below is the complete view.\n");
    }
    out
}

/// The flow's calls by form and its constructs not drawn, nested flows
/// included, counted through [`Flow::each_call`] as the Calls list is.
pub(crate) fn coverage(flow: &Flow) -> String {
    fn not_drawn(steps: &[Step], into: &mut BTreeMap<NotDrawn, usize>) {
        for s in steps {
            match s {
                Step::Nested { flow, .. } => {
                    for (k, n) in &flow.not_drawn {
                        *into.entry(*k).or_default() += n;
                    }
                    not_drawn(&flow.steps, into);
                }
                Step::Branch { arms, .. } => {
                    for a in arms {
                        not_drawn(&a.pre, into);
                        not_drawn(&a.body, into);
                        not_drawn(&a.after, into);
                    }
                }
                Step::Loop {
                    pre,
                    body,
                    post,
                    orelse,
                    ..
                } => {
                    not_drawn(pre, into);
                    not_drawn(body, into);
                    not_drawn(post, into);
                    not_drawn(orelse, into);
                }
                Step::Finally { body, .. } => not_drawn(body, into),
                Step::Call(_) | Step::Exit { .. } => {}
            }
        }
    }
    let mut n = [0; 6];
    flow.each_call(&mut |_, c| {
        n[0] += 1;
        n[match c.target {
            _ if c.local.is_some() => 5,
            Target::Resolved(_) => 1,
            Target::OneOf { .. } => 2,
            Target::Outside => 3,
            Target::Hidden => 4,
        }] += 1;
    });
    let mut kinds = flow.not_drawn.clone();
    not_drawn(&flow.steps, &mut kinds);
    for k in undrawn(flow).into_values() {
        *kinds.entry(k).or_default() += 1;
    }
    let kinds: Vec<String> = kinds
        .iter()
        .map(|(k, n)| format!("{n} {}", k.label()))
        .collect();
    // A call of a closure or nested function drawn in the chart.
    let local = if n[5] > 0 {
        format!(", {} local", n[5])
    } else {
        String::new()
    };
    let mut out = format!(
        "Coverage: {} calls: {} resolved, {} one of, {} outside, {} hidden{local}. Not drawn: {}.",
        n[0],
        n[1],
        n[2],
        n[3],
        n[4],
        if kinds.is_empty() {
            "none".to_string()
        } else {
            kinds.join(", ")
        }
    );
    if flow.syntax_errors {
        out.push_str(" Syntax errors: chart may be incomplete.");
    }
    out
}

#[cfg(test)]
mod tests;
