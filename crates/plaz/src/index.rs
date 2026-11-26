//! The index pages: every charted function from the top down, re-listing
//! only the calls the pages read as certain. `map/index.md` lists the top
//! directories and holds the functions of the files at the top of the
//! tree; each top directory's functions are on a page of its own.

use crate::model::{FnId, Project, Target};
use crate::names::{Names, INDEX};
use crate::render::{inline_code, link, link_text, yaml_str, Page};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const INTRO: &str = "Every charted function, from the top down, following only the calls the map reads as certain (a single link in a page's **Calls**); a call that may be one of several functions is not followed. Each function is expanded once, at the shallowest depth the first section reaches it, or else where a later section first reaches it; its last line, \"also calls\", lists the functions it calls that are not expanded under it. A function listed as not called may still be called through a value, a trait or virtual call, a macro, an attribute, or from outside the project.";

pub struct Index {
    /// Empty when no function is charted.
    pub pages: Vec<Page>,
    /// Per function reached from a function no charted function calls:
    /// the depth at which the index expands it (0 for that one) and the id
    /// of the index page that does.
    pub levels: BTreeMap<FnId, (usize, String)>,
}

/// Where each function is expanded: the function it is listed under
/// (none for the first of a tree), its depth, and the first of its tree.
type Placed<'a> = BTreeMap<&'a FnId, (Option<&'a FnId>, usize, &'a FnId)>;

/// Places each of `starts` not yet placed at depth 0, then breadth first
/// the charted functions they certainly call.
fn place<'a>(starts: &[&'a FnId], callees: &'a BTreeMap<FnId, Vec<FnId>>, placed: &mut Placed<'a>) {
    let mut queue = VecDeque::new();
    for &s in starts {
        if !placed.contains_key(s) {
            placed.insert(s, (None, 0, s));
            queue.push_back(s);
        }
    }
    while let Some(f) = queue.pop_front() {
        let (_, depth, start) = placed[f];
        for c in &callees[f] {
            if callees.contains_key(c) && !placed.contains_key(c) {
                placed.insert(c, (Some(f), depth + 1, start));
                queue.push_back(c);
            }
        }
    }
}

/// A line of an index list: a function under its parent at a depth, or
/// the "also calls" line closing a function's list.
enum Line<'a> {
    Fn(&'a FnId, Option<&'a FnId>, usize),
    Also(usize, Vec<String>),
}

/// The top directory of `path`, `""` at the top of the tree.
fn top_dir(path: &str) -> &str {
    path.split_once('/').map_or("", |d| d.0)
}

pub fn index(p: &Project, names: &Names) -> Index {
    let item = |f: &FnId| &p.files[&f.path].code.items[f.item];
    let is_main = |f: &FnId| item(f).name == "main" && item(f).owner.is_none();
    // Each charted function's certain callees, each once, in Calls order.
    let mut callees = BTreeMap::new();
    for (path, file) in &p.files {
        for (&i, flow) in &file.flows {
            let (mut to, mut seen) = (Vec::new(), BTreeSet::new());
            flow.each_call(&mut |_, c| {
                if let Target::Resolved(t) = &c.target {
                    if seen.insert(t) {
                        to.push(t.clone());
                    }
                }
            });
            let f = FnId {
                path: path.clone(),
                item: i,
            };
            callees.insert(f, to);
        }
    }
    if callees.is_empty() {
        return Index {
            pages: Vec::new(),
            levels: BTreeMap::new(),
        };
    }
    let with_main: BTreeSet<&str> = (callees.keys().filter(|f| is_main(f)))
        .map(|f| f.path.as_str())
        .collect();
    let mut fns: Vec<&FnId> = callees.keys().collect();
    fns.sort_by_key(|f| {
        (
            !with_main.contains(f.path.as_str()),
            &f.path,
            !is_main(f),
            f.item,
        )
    });
    let charted = |t: &FnId| callees.contains_key(t);
    let uncalled = |f: &FnId| names.callers(f).is_empty();
    let calls_one = |f: &FnId| !callees[f].is_empty();
    let pick = |keep: &dyn Fn(&FnId) -> bool| -> Vec<&FnId> {
        fns.iter().copied().filter(|f| keep(f)).collect()
    };
    let roots = pick(&|f| uncalled(f) && calls_one(f));
    let isolated = pick(&|f| uncalled(f) && !calls_one(f));
    let maybe = pick(&|f| !uncalled(f) && names.callers(f).iter().all(|c| !c.2));
    let mut placed = Placed::new();
    place(&roots, &callees, &mut placed);
    let levels = (placed.iter())
        .map(|(f, (_, d, s))| {
            (
                (*f).clone(),
                (*d, names.index_page(top_dir(&s.path)).to_string()),
            )
        })
        .collect();
    place(&maybe, &callees, &mut placed);
    let mut cycles = Vec::new();
    for &f in &fns {
        if uncalled(f) || placed.contains_key(f) {
            continue;
        }
        // Up the first unplaced certain caller until one repeats: that
        // one is on a cycle, and `f` is in its tree.
        let (mut start, mut seen) = (f, BTreeSet::new());
        while seen.insert(start) {
            let up = (names.callers(start).iter().filter(|c| c.2))
                .filter_map(|c| callees.get_key_value(&c.0).map(|k| k.0))
                .find(|c| !placed.contains_key(c));
            let Some(up) = up else { break };
            start = up;
        }
        place(&[start], &callees, &mut placed);
        cycles.push(start);
    }

    // Per top directory: its page's text after the front matter, the
    // count of functions on it, and the section and file last headed.
    let mut bodies: BTreeMap<&str, (String, usize, &str, &str)> = BTreeMap::new();
    for (heading, entries) in [
        ("Not called by any charted function", &roots),
        ("Called only by calls that may be another function", &maybe),
        ("In a call cycle no function above reaches", &cycles),
        ("Not called, calling nothing certain", &isolated),
    ] {
        for &e in entries {
            let (out, count, last, file) = bodies.entry(top_dir(&e.path)).or_default();
            if *last != heading {
                (*last, *file) = (heading, "");
                out.push_str(&format!("\n## {heading}\n"));
            }
            if *file != e.path {
                *file = &e.path;
                let page = names.page(&e.path);
                out.push_str(&format!("\n### [[{page}|{}]]\n", link_text(&e.path)));
            }
            // Depth first, as the list reads.
            let mut stack = vec![Line::Fn(e, None, 0)];
            while let Some(line) = stack.pop() {
                let (f, parent, depth) = match line {
                    Line::Fn(f, parent, depth) => (f, parent, depth),
                    Line::Also(depth, links) => {
                        let pad = "  ".repeat(depth);
                        out.push_str(&format!("{pad}- also calls: {}\n", links.join(", ")));
                        continue;
                    }
                };
                out.push_str(&format!("{}- {}\n", "  ".repeat(depth), link(p, names, f)));
                *count += usize::from(charted(f));
                if placed.get(f).is_none_or(|at| at.0 != parent) {
                    continue;
                }
                let (under, also): (Vec<&FnId>, Vec<&FnId>) =
                    (callees[f].iter()).partition(|c| !charted(c) || placed[c].0 == Some(f));
                if !also.is_empty() {
                    let links = also.into_iter().map(|c| link(p, names, c)).collect();
                    stack.push(Line::Also(depth + 1, links));
                }
                for c in under.into_iter().rev() {
                    stack.push(Line::Fn(c, Some(f), depth + 1));
                }
            }
        }
    }

    let head = |title: &str| {
        format!(
            "---\ntitle: {}\ntags: [\"plaz\"]\nkind: map\n---\n",
            yaml_str(title)
        )
    };
    let mut index = head("Index") + INTRO + "\n";
    let mut pages = Vec::new();
    let mut listed = String::new();
    for (dir, (body, count, ..)) in &bodies {
        if dir.is_empty() {
            continue;
        }
        let id = names.index_page(dir);
        let s = if *count == 1 { "" } else { "s" };
        listed.push_str(&format!(
            "- [[{id}|{}]] · {count} function{s} listed\n",
            link_text(dir)
        ));
        let under = inline_code(&format!("{dir}/"));
        let text = format!(
            "{}The trees that start in a file under {under}, from the [[{INDEX}|index]]. A tree follows certain calls into other directories; a function of {under} first reached from another directory is listed on that directory's page. {INTRO}\n{body}",
            head(&format!("Index · {dir}")),
        );
        pages.push(Page {
            rel_path: format!("{id}.md"),
            content: text,
        });
    }
    if !listed.is_empty() {
        index.push_str(&format!("\n## Directories\n{listed}"));
    }
    if let Some((body, ..)) = bodies.get("") {
        index.push_str(body);
    }
    pages.push(Page {
        rel_path: format!("{INDEX}.md"),
        content: index,
    });
    Index { pages, levels }
}
