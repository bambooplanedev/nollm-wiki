//! The decision flowchart of one function and its coverage line.

use crate::model::{Comment, ExitKind, Flow, FnId, NotDrawn, Step, Target};
use std::collections::BTreeMap;

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
        let (open, close) = shape;
        self.lines
            .push(format!("    n{id}{open}\"{}\"{close}", mermaid_label(text)));
        self.edges_to(from, id);
        id
    }

    fn edges_to(&mut self, from: &[(usize, Option<&str>)], to: usize) {
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

    /// `steps` as a chart of their own, beside the path.
    fn beside(&mut self, name: &str, steps: &[Step]) {
        let id = self.id();
        self.lines
            .push(format!("    subgraph n{id} [\"{}\"]", mermaid_label(name)));
        self.flow(name, steps);
        self.lines.push("    end".to_string());
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
            Step::Call(c) => {
                let shape = match &c.target {
                    Target::Resolved(_) => ("[", "]"),
                    Target::OneOf { .. } => ("[[", "]]:::oneof"),
                    Target::Outside => ("[/", "/]"),
                    Target::Hidden => (">", "]:::hidden"),
                };
                let id = self.node(shape, &c.text, &ends);
                if let Target::Resolved(t) = &c.target {
                    let href = (self.href)(t);
                    self.clicks.push(format!("    click n{id} href \"{href}\""));
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
            Step::Branch { .. } => self.branched(s, ends),
            Step::Nested { name, flow, .. } => {
                self.beside(name, &flow.steps);
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
            let (_, at) = self.steps(pre, back);
            let d = self.node(shape, header, &at);
            self.note(comment.as_ref(), d);
            self.edges_to(&[(d, Some("loop"))], entry.unwrap_or(d));
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

/// The fenced `mermaid` block of `flow`, named `name`, in the file at
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
    };
    c.flow(name, &flow.steps);
    let mut body = String::from("flowchart TD\n");
    for l in c.lines.iter().chain(&c.clicks) {
        body.push_str(l);
        body.push('\n');
    }
    body.push_str(
        "    classDef oneof stroke-dasharray: 5 5\n    classDef hidden stroke-dasharray: 2 2\n",
    );
    let mut out = format!("```mermaid\n{body}```\n");
    if c.edges > 500 || body.chars().count() > 50_000 {
        out.push_str("\nThis chart exceeds Mermaid's default limits (500 edges, 50,000 characters); the Calls list below is the complete view.\n");
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
    let mut n = [0; 5];
    flow.each_call(&mut |_, c| {
        n[0] += 1;
        n[match c.target {
            Target::Resolved(_) => 1,
            Target::OneOf { .. } => 2,
            Target::Outside => 3,
            Target::Hidden => 4,
        }] += 1;
    });
    let mut kinds = flow.not_drawn.clone();
    not_drawn(&flow.steps, &mut kinds);
    let kinds: Vec<String> = kinds
        .iter()
        .map(|(k, n)| format!("{n} {}", k.label()))
        .collect();
    let mut out = format!(
        "Coverage: {} calls: {} resolved, {} one of, {} outside, {} hidden. Not drawn: {}.",
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
mod tests {
    use super::*;
    use crate::flow::testutil::{arm, branch, call, exit, flow_of, looping};
    use crate::model::NotDrawn;

    /// The node and edge lines of the chart of `steps`.
    fn drawn(steps: Vec<Step>) -> Vec<String> {
        chart("f", "a.c", &flow_of(steps), &|_| String::new())
            .lines()
            .filter(|l| l.starts_with("    n"))
            .map(|l| l.trim().to_string())
            .collect()
    }

    fn a_breakable_k(inside: Vec<Step>) -> Step {
        looping(
            "go",
            vec![
                branch(
                    vec![
                        arm(Some("k"), inside, false),
                        arm(None, vec![call("b")], false),
                    ],
                    true,
                ),
                call("c"),
            ],
            Vec::new(),
            false,
        )
    }

    #[test]
    fn a_break_in_a_breakable_branch_lands_on_what_follows_the_branch() {
        let steps = vec![a_breakable_k(vec![call("a"), exit(ExitKind::Break(None))])];
        assert_eq!(
            drawn(steps),
            [
                "n0([\"f\"])",
                "n1{\"go\"}",
                "n0 --> n1",
                "n2{\"k\"}",
                "n1 -->|loop| n2",
                "n3>\"a()\"]:::hidden",
                "n2 -->|yes| n3",
                "n4[\"exit\"]",
                "n3 --> n4",
                "n5>\"b()\"]:::hidden",
                "n2 -->|no| n5",
                "n6>\"c()\"]:::hidden",
                "n5 --> n6",
                "n4 --> n6",
                "n6 --> n1",
                "n7([\"end\"])",
                "n1 -->|done| n7",
            ]
        );
    }

    #[test]
    fn a_continue_in_a_breakable_branch_goes_to_the_loop() {
        let steps = vec![a_breakable_k(vec![
            call("a"),
            exit(ExitKind::Continue(None)),
        ])];
        assert_eq!(
            drawn(steps),
            [
                "n0([\"f\"])",
                "n1{\"go\"}",
                "n0 --> n1",
                "n2{\"k\"}",
                "n1 -->|loop| n2",
                "n3>\"a()\"]:::hidden",
                "n2 -->|yes| n3",
                "n4[\"exit\"]",
                "n3 --> n4",
                "n4 --> n1",
                "n5>\"b()\"]:::hidden",
                "n2 -->|no| n5",
                "n6>\"c()\"]:::hidden",
                "n5 --> n6",
                "n6 --> n1",
                "n7([\"end\"])",
                "n1 -->|done| n7",
            ]
        );
    }

    #[test]
    fn a_break_in_a_loop_inside_a_breakable_branch_leaves_that_loop() {
        let inner = looping(
            "again",
            vec![exit(ExitKind::Break(None))],
            Vec::new(),
            false,
        );
        let steps = vec![
            branch(vec![arm(Some("k"), vec![inner, call("w")], false)], true),
            call("c"),
        ];
        assert_eq!(
            drawn(steps),
            [
                "n0([\"f\"])",
                "n1{\"k\"}",
                "n0 --> n1",
                "n2{\"again\"}",
                "n1 -->|yes| n2",
                "n3[\"exit\"]",
                "n2 -->|loop| n3",
                "n4>\"w()\"]:::hidden",
                "n3 --> n4",
                "n2 -->|done| n4",
                "n5>\"c()\"]:::hidden",
                "n4 --> n5",
                "n1 -->|no| n5",
                "n6([\"end\"])",
                "n5 --> n6",
            ]
        );
    }

    #[test]
    fn a_labelled_break_passes_through_a_breakable_branch() {
        let mut outer = a_breakable_k(vec![exit(ExitKind::Break(Some("outer".into())))]);
        if let Step::Loop { label, .. } = &mut outer {
            *label = Some("outer".into());
        }
        assert_eq!(
            drawn(vec![outer]),
            [
                "n0([\"f\"])",
                "n1{\"go\"}",
                "n0 --> n1",
                "n2{\"k\"}",
                "n1 -->|loop| n2",
                "n3[\"exit\"]",
                "n2 -->|yes| n3",
                "n4>\"b()\"]:::hidden",
                "n2 -->|no| n4",
                "n5>\"c()\"]:::hidden",
                "n4 --> n5",
                "n5 --> n1",
                "n6([\"end\"])",
                "n3 --> n6",
                "n1 -->|done| n6",
            ]
        );
    }

    #[test]
    fn a_falling_arm_joins_the_next_arms_body_and_not_the_join() {
        let steps = vec![
            branch(
                vec![
                    arm(Some("1"), vec![call("a")], true),
                    arm(Some("2"), vec![call("b")], true),
                ],
                false,
            ),
            call("c"),
        ];
        assert_eq!(
            drawn(steps),
            [
                "n0([\"f\"])",
                "n1{\"1\"}",
                "n0 --> n1",
                "n2>\"a()\"]:::hidden",
                "n1 -->|yes| n2",
                "n3{\"2\"}",
                "n1 -->|no| n3",
                "n4>\"b()\"]:::hidden",
                "n3 -->|yes| n4",
                "n2 --> n4",
                "n5>\"c()\"]:::hidden",
                "n4 --> n5",
                "n3 -->|no| n5",
                "n6([\"end\"])",
                "n5 --> n6",
            ]
        );
    }

    #[test]
    fn a_test_after_loop_enters_its_body_and_tests_after_it() {
        let body = vec![
            call("a"),
            branch(
                vec![arm(Some("k"), vec![exit(ExitKind::Continue(None))], false)],
                false,
            ),
            call("b"),
        ];
        let steps = vec![looping("go", body, Vec::new(), true)];
        assert_eq!(
            drawn(steps),
            [
                "n0([\"f\"])",
                "n1>\"a()\"]:::hidden",
                "n0 --> n1",
                "n2{\"k\"}",
                "n1 --> n2",
                "n3[\"exit\"]",
                "n2 -->|yes| n3",
                "n4>\"b()\"]:::hidden",
                "n2 -->|no| n4",
                "n5{\"go\"}",
                "n4 --> n5",
                "n3 --> n5",
                "n5 -->|loop| n1",
                "n6([\"end\"])",
                "n5 -->|done| n6",
            ]
        );
    }

    #[test]
    fn a_post_runs_after_the_body_and_a_continue_before_the_header() {
        let body = vec![
            call("a"),
            branch(
                vec![arm(Some("k"), vec![exit(ExitKind::Continue(None))], false)],
                false,
            ),
            call("b"),
        ];
        let steps = vec![looping("go", body, vec![call("u")], false)];
        assert_eq!(
            drawn(steps),
            [
                "n0([\"f\"])",
                "n1{\"go\"}",
                "n0 --> n1",
                "n2>\"a()\"]:::hidden",
                "n1 -->|loop| n2",
                "n3{\"k\"}",
                "n2 --> n3",
                "n4[\"exit\"]",
                "n3 -->|yes| n4",
                "n5>\"b()\"]:::hidden",
                "n3 -->|no| n5",
                "n6>\"u()\"]:::hidden",
                "n5 --> n6",
                "n4 --> n6",
                "n6 --> n1",
                "n7([\"end\"])",
                "n1 -->|done| n7",
            ]
        );
    }

    #[test]
    fn calls_and_closures_in_a_post_are_counted() {
        let mut nested = flow_of(vec![call("n")]);
        nested.not_drawn.insert(NotDrawn::Closure, 1);
        let post = vec![
            call("u"),
            Step::Nested {
                name: "g".into(),
                line: 1,
                flow: nested,
            },
        ];
        let flow = flow_of(vec![looping("go", vec![call("a")], post, false)]);
        assert_eq!(
            coverage(&flow),
            "Coverage: 3 calls: 0 resolved, 0 one of, 0 outside, 3 hidden. Not drawn: 1 closure."
        );
    }

    #[test]
    fn relative_page_ids_climb_only_past_the_shared_directories() {
        assert_eq!(relative("map/src/x/a_rs", "map/src/b_rs"), "../b_rs");
        assert_eq!(relative("map/src/a_rs", "map/src/a_rs"), "a_rs");
        assert_eq!(relative("map/a_rs", "map/src/x/b_rs"), "src/x/b_rs");
        assert_eq!(relative("map/x/a_rs", "map/y/x"), "../y/x");
    }
}
