//! The conditional directive groups of a first-branch tree's text.

use crate::support::one_line;
use crate::tree_sitter::Node;
use std::collections::BTreeSet;
use std::ops::Range;

/// A conditional directive group in the text of a first-branch tree: its
/// `#if…` line as written, where that line starts (a byte, a 1-based
/// line), and the bytes of its first branch.
pub(in crate::links::flow) struct Group {
    pub cond: String,
    pub at: usize,
    pub line: usize,
    pub body: Range<usize>,
    /// The first branch runs to the `#endif`: no `#elif` or `#else`.
    pub only: bool,
}

/// The conditional directive groups with a line starting in `range` of
/// `text`, nested ones each once: the first-branch tree shows each one's
/// first branch as if unconditional.
pub(in crate::links::flow) fn directive_groups(text: &str, range: Range<usize>) -> Vec<Group> {
    let (mut open, mut seen, mut at): (Vec<usize>, BTreeSet<usize>, usize) =
        (Vec::new(), BTreeSet::new(), 0);
    let mut all: Vec<Group> = Vec::new();
    for (row, line) in text.split_inclusive('\n').enumerate() {
        let word = line.trim_start().strip_prefix('#').map(|r| {
            r.trim_start()
                .split(|c: char| !c.is_ascii_alphabetic())
                .next()
                .unwrap_or("")
        });
        let last = matches!(
            word,
            Some("elif" | "elifdef" | "elifndef" | "else" | "endif")
        );
        // A group's first branch ends at its first `#elif`, `#else` or `#endif`.
        if let Some(&g) = open.last().filter(|_| last) {
            let g = &mut all[g];
            g.body.end = g.body.end.min(at);
            g.only &= word == Some("endif") && g.body.end == at;
        }
        let group = match word {
            Some("if" | "ifdef" | "ifndef") => {
                all.push(Group {
                    cond: one_line(line.split("//").next().unwrap_or(line)),
                    at,
                    line: row + 1,
                    body: at + line.len()..usize::MAX,
                    only: true,
                });
                open.push(all.len() - 1);
                open.last().copied()
            }
            Some("elif" | "elifdef" | "elifndef" | "else") => open.last().copied(),
            Some("endif") => open.pop(),
            _ => None,
        };
        if range.contains(&at) {
            seen.extend(group);
        }
        at += line.len();
    }
    let all = all.into_iter().enumerate();
    all.filter(|(i, _)| seen.contains(i)).map(|g| g.1).collect()
}

/// The outermost of `groups` whose `#if…` line is in `within` and whose
/// first branch holds the node `n`, with its index.
pub(in crate::links::flow) fn holding<'g>(
    groups: &'g [Group],
    n: Node,
    within: &Range<usize>,
) -> Option<(usize, &'g Group)> {
    let holds = |g: &&Group| {
        within.start <= g.at
            && g.body.end <= within.end
            && g.body.start <= n.start_byte()
            && n.end_byte() <= g.body.end
    };
    groups
        .iter()
        .enumerate()
        .filter(|g| holds(&g.1))
        .min_by_key(|g| g.1.at)
}
