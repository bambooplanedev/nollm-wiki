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

struct Chart<'a> {
    path: &'a str,
    href: &'a dyn Fn(&FnId) -> String,
    lines: Vec<String>,
    clicks: Vec<String>,
    next: usize,
    edges: usize,
    /// The loops around the step being drawn, innermost last: label,
    /// node, and the ends its `break`s leave open.
    loops: Vec<(Option<String>, usize, Ends)>,
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
        let (_, ends) = self.steps(steps, vec![(start, None)]);
        if !ends.is_empty() {
            self.node(("([", "])"), "end", &ends);
        }
        self.loops = loops;
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
            Some(l) => self.loops.iter().rposition(|lp| lp.0.as_ref() == Some(l)),
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
                        if let Some(i) = self.target(label.as_ref()) {
                            self.loops[i].2.push((id, None));
                        }
                    }
                    ExitKind::Continue(label) => {
                        let id = self.node(("[", "]"), text, &ends);
                        if let Some(i) = self.target(label.as_ref()) {
                            let to = self.loops[i].1;
                            self.edges_to(&[(id, None)], to);
                        }
                    }
                }
                Vec::new()
            }
            Step::Assign { target, value, .. } => {
                vec![(
                    self.node(("[", "]"), &format!("{target} = {value}"), &ends),
                    None,
                )]
            }
            Step::Loop {
                header,
                comment,
                pre,
                body,
                label,
                endless,
                hidden_exit,
                orelse,
                ..
            } => {
                let shape = if *endless { ("[", "]") } else { ("{", "}") };
                // A header run before every iteration is where each
                // iteration starts.
                let (entry, at) = self.steps(pre, ends);
                let d = self.node(shape, header, &at);
                let again = entry.unwrap_or(d);
                self.note(comment.as_ref(), d);
                self.loops.push((label.clone(), again, Vec::new()));
                let (_, back) = self.steps(body, vec![(d, Some("loop"))]);
                self.edges_to(&back, again);
                let mut out = self.loops.pop().map(|lp| lp.2).unwrap_or_default();
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
            Step::Branch {
                head,
                arms,
                exhaustive,
                ..
            } => {
                let mut no = ends;
                if let Some(h) = head {
                    no = vec![(self.node(("[", "]"), h, &no), None)];
                }
                let mut out = Vec::new();
                for a in arms {
                    let (_, at) = self.steps(&a.pre, no);
                    no = if let Some(cond) = &a.cond {
                        let d = self.node(("{", "}"), cond, &at);
                        self.note(a.comment.as_ref(), d);
                        let (_, ends) = self.steps(&a.body, vec![(d, Some("yes"))]);
                        out.extend(self.steps(&a.after, ends).1);
                        vec![(d, Some("no"))]
                    } else {
                        // No diamond: the note hangs off the body's
                        // first node, or the diamond that leads here.
                        let (entry, ends) = self.steps(&a.body, at.clone());
                        out.extend(self.steps(&a.after, ends).1);
                        if let Some(to) = entry.or_else(|| at.first().map(|e| e.0)) {
                            self.note(a.comment.as_ref(), to);
                        }
                        Vec::new()
                    };
                }
                if !*exhaustive {
                    out.extend(no);
                }
                out
            }
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
                    pre, body, orelse, ..
                } => {
                    not_drawn(pre, into);
                    not_drawn(body, into);
                    not_drawn(orelse, into);
                }
                Step::Finally { body, .. } => not_drawn(body, into),
                Step::Call(_) | Step::Exit { .. } | Step::Assign { .. } => {}
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

    #[test]
    fn relative_page_ids_climb_only_past_the_shared_directories() {
        assert_eq!(relative("map/src/x/a_rs", "map/src/b_rs"), "../b_rs");
        assert_eq!(relative("map/src/a_rs", "map/src/a_rs"), "a_rs");
        assert_eq!(relative("map/a_rs", "map/src/x/b_rs"), "src/x/b_rs");
        assert_eq!(relative("map/x/a_rs", "map/y/x"), "../y/x");
    }
}
