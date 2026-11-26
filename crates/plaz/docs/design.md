# plaz — design

Approved 2026-11-13.

Status: function charts for Rust, Python and C/C++, call targets, index pages, `plaz build` and `plaz setup` exist (a preview, installed with nollm-wiki). State diagrams, "what changed in the logic", the other languages and publishing are planned; each is marked as planned where it appears below.

## Why

Large projects written mostly by coding agents are too big to hold in one head, and the owner did not write the code. Bugs surface late, at manual checks, and that is where technical debt grows. The worst ones are not broken wiring but wrong logic: an action runs "because X", and X is wrong, incomplete, or only valid under other conditions, yet the action runs every time.

plaz shows what the code really does, so that:

- the owner reads each decision with its literal condition in a browser and sees "that is not what I meant";
- an agent, after its change, sees which conditions and transitions changed (planned: see "What changed in the logic").

The map never judges whether a "because" is right. It shows it; the human judges. It shows changes; it does not enforce acknowledgement.

## A separate tool from `wiki`

`wiki` is for agents: its measure is whether an agent finds exact `path:start-end` context fast. plaz is for people: its measure is whether a person understands the logic correctly. Mixing them would let map changes move the measured agent behaviour of `wiki`, and one README would promise two products.

plaz depends on `wiki-code` only: a crate in this workspace, used by path, that holds the code model (file walking, parsing, the item model, call sites, the candidate index used to find call targets, comment text). The pages, search and MCP tools of `wiki` live in the `wiki` crate, which plaz does not use, and `wiki` knows nothing about the map. One set of tree-sitter grammar versions, pinned exactly in `wiki-code`, serves both. A compiled wiki is optional: the map is built from sources alone. When a compiled wiki vault is present, references to code become wikilinks to its code pages; without it they are plain `path:start-end`. Navigation between charts uses the map's own pages and never needs the wiki.

## What it shows

No end-to-end flows and no entry points: the map never claims which path a run takes through the functions. Each function gets a section with:

1. **Level**, right under the signature, only for a function the index expands under a function no charted function calls: the depth at which it does (0 for that one), as `Level 2 in the index`, "index" linking to that index page.
2. **Locals**, under the signature and the level line: the names the body declares, parameters left out (they are in the signature), sorted by name, each with its type where the code writes one (`item: Entry`). Python `global` and `nonlocal` names go on an `Outside names:` line instead. No line when there are none. A nested function's or closure's names stay with its own sub-chart and are not listed.
3. **Called from**: the functions that call it, each linking to its chart, split into "certainly calls" and "may call (if this `get` is this one)".
4. **Decision flowchart** (UML activity, Mermaid) of the body: every `if` / `else if` / `switch` / `match` arm is a diamond holding the literal condition code and the whole comment attached to it; actions and calls in order; call nodes are clickable where the renderer allows.
5. **Calls**: the callees as a plain wikilink list (guaranteed navigation in Obsidian and Quartz).

The person builds the path by moving up and down between charts. The map makes no claim it cannot back.

**Index pages** (owner decision, 2026-11-26) are the way in from the top. They re-list, top down, only the links the function sections already state as certain (a single link in **Calls**), and claim nothing of their own: cross-function edges are where a map lies, so no edge is added, guessed or followed through "one of". `map/index.md` lists the top directories, each with the count of functions its page lists, and carries the trees that start in the files at the top of the tree; each top directory has `map/index/<dir>.md`, holding the trees that start in its files. A page has up to four sections, each grouped by file:

- **Not called by any charted function**: these functions, each with what it certainly calls nested under it, in the order of its Calls list.
- **Called only by calls that may be another function**: the functions whose only callers are "may call", the same way.
- **In a call cycle no function above reaches**: what is left. Each tree starts on a cycle: from the first function left, in the order below, up its first certain caller until one repeats; that one starts the tree.
- **Not called, calling nothing certain**: a flat list.

Files with a `main` function come first, then by path; in a file `main` comes first, then source order. A breadth-first walk over all the trees, in that order, places each function once: at the shallowest depth the first section's trees reach it, or else where a later section's walk first reaches it (the second section's trees together, then each cycle tree in turn); it is expanded there, and every other function that certainly calls it lists it on its own last line, "also calls" (recursion included). A tree follows calls into other directories; it sits on the page of the directory where its first function is. The page says that "not called" is only what the map sees: a function may still be called through a value, a trait or virtual call, a macro, an attribute, or from outside the project. No "entry point" wording, no sequence diagrams, no path claims beyond these links.

**State diagrams** (planned, not built), simple shape only: an enum-typed field assigned enum literals, with the guard from the enclosing branch and the method or arm as the event. Shown as a *candidate* state machine. Other shapes (computed targets, string-literal unions, typestate) get a "not covered" note. The owner states that the order state machine of a C++ trading system is implemented fully in this shape (not checked yet).

## Call targets: four forms, never a guess

The code is deterministic, so an undetermined target is written as what it is: "either … or …".

| Form | When | Shown as |
|---|---|---|
| **Resolved** | qualified path that resolves, `self`/`Self` receiver with no override in a subclass, or a receiver of known type, with one target | a link |
| **One of** | member call on a receiver of unknown type with project candidates, or `self` call with overrides | "one of: A, B, or outside the project", each a link |
| **Outside** | path or receiver type known to be external (std, dependency, an import that resolves to no project file) | "outside the project" |
| **Hidden** | call to a parameter, local, closure, function value, unknown `Q::` path, nested def | "target not visible", never "outside", never "one of" |

The node text is always the literal call (`ask(messages)`), on one line; past 80 characters its middle is cut to `…`, so its start and its end stay visible (the Calls list shows the same text). The form only decides the link. When unsure: Hidden.

A closure, nested function or lambda with control flow of its own is a sub-chart beside the path. Dotted edges tie it to the path, saying where it is used and nothing about when or how often it runs: `passed to`, from the sub-chart to the call that takes it as an argument (to the innermost call whose arguments hold it); `calls`, from a call of it by name to the sub-chart. A call by name counts only when that name is bound once in the whole function, nested flows included (no parameter, loop variable, second `let` or item of that name), and the call stands where the binding is in scope: after a Rust `let` or a C++ declaration, in the rest of its block; in the whole block of a Rust nested `fn`; from a Python nested `def` (undecorated, in no class) to the end of the function around it. Such a call is drawn as a local call (a rounded box), and a call naming it as an argument (`v.sort_by_key(fits)`, `filter(keep, rows)`) gets a `passed to` edge too. Its Calls line reads "the closure `fits`, drawn in this chart" (`lambda`, `nested function`), and the coverage line counts it as `local`. It is no call target: Called from and the index pages do not list it. Any other call of a local stays Hidden.

A call adds a node of its own only when it says something the nodes around it do not (owner decision, 2026-11-27). A step of a method chain outside the project is not drawn when a longer call of the chart starts with its text: `a.b().c().d()` is one node, the outermost, whose text holds the steps (so is a "target not visible" step whose receiver is such a step). A step that links into the project, or reads "one of", stays a node of its own, as does one that takes a closure drawn in the chart. Rust's `Some(…)`, `Ok(…)` and `Err(…)` outside the project build a value and are not drawn; the calls in their arguments are. The Calls list keeps every call, and the coverage line counts what is not drawn (`chain step`, `Some/Ok/Err`).

Each chart carries a coverage line: counts of resolved / one of / outside / hidden calls (and local ones, when there are), plus branches and code not drawn (Rust `?`, `unwrap_or`, Rust `assert!`-family macros and Python `assert` (counted as `assert`, no node), conditions inside closures or ternaries, C/C++ `#if` branches, `goto` and jumps hidden in macros, chain steps and wrappers not drawn).

## What changed in the logic (planned, not built)

Against any commit, default HEAD (uncommitted work vs last commit). plaz would extract the old tree from a `git archive` of that commit into a temporary directory and compare by node identity = function + condition text + arm order (set difference, no tree matching). The output would be a page for people, and the same text in the terminal. Git is the history; no map history is stored.

## Output

Markdown pages with Mermaid, written by default into `<source>/.nollm-wiki/map/`, beside the pages of `wiki`, or into a vault given with `--vault`, with references to code as wikilinks to the wiki's code pages when that vault has them and as plain `path:start-end` otherwise. Quartz builds them into a browser site (the main human view); Obsidian opens them as a vault. The pages are for people: `plaz setup` tells agents nothing about them. plaz emits Quartz-compatible content; it does not ship or run Quartz. One vault holds the map of one source tree: a build that finds no code file warns and leaves the vault as it is.

## Non-goals

- No model, no paraphrase of conditions, no guessing of intent, no flagging of "suspicious" logic.
- No end-to-end flows, entry points, or sequence diagrams: navigation between charts replaces them, and the index pages re-list certain links from the top down, claiming no path beyond them.
- No commit/PR diff output (skipped for now).
- No stored map history.
- No project specs as a source: the map must work on any repo.
- No precise name resolution of our own (Sourcetrail and GitHub stack-graphs both died on per-language upkeep).
- No cross-language links.
- No changes to the behaviour of `wiki`, its search, hints or MCP tools.

## Anti-goal

A misleading map: a person reads it and comes away with an understanding that does not match what the project does. If the map cannot be sure, it says so in place; an honest "not visible" beats a confident wrong link.

## Decisions

Decided by the owner:
- Audience: the owner first, the agent second.
- Literal code and comments only, no model; doubt shown as "either … or …".
- Logic inside functions plus caller/callee navigation.
- Index pages from the top down (2026-11-26), re-listing certain links only; this revisits the earlier "no entry points" rule, whose reason stands.
- Lighter charts (2026-11-27): chain steps and `Some`/`Ok`/`Err` wrappers outside the project are not drawn, the Calls list and the coverage line still name them; this revisits the earlier "charts complete, no collapsing" rule.
- State diagrams in the simple shape; sequence diagrams deferred.
- Browser main view (Quartz); Obsidian and agent access too.
- No stored history; compare with commits on demand.
- A tool of its own, depending on the code layer of `wiki` (`wiki-code`) as a library; it began in a repository of its own and is now a crate of this workspace, depending on it by path.

Made during design (open to change):
- Change baseline HEAD by default.
- Comments shown whole up to a line cap, then a link to the lines; never cut to the first sentence.
- Charts complete in what they decide: every decision, exit and call that links or reads "one of" is a node; only calls that repeat what another node says are left out or put together (see "Call targets"), and the Calls list stays the complete view. No collapsing of large functions: a chart over Mermaid's 50,000-character limit is not drawn, and a notice points to the Calls list.
- Python nested defs and closures drawn as their own sub-charts.
- Mermaid output escaped (quotes, `{}`, `<>`, `|`, `\`, `:`, `$`) and validated with mermaid-cli in tests; clicks checked in Quartz and Obsidian, with the wikilink list as the fallback.
- Measurement gate before release, on projects not used to design this: zero wrong "resolved" links, every "one of" contains the true target or is "hidden", every diamond's text equals the source.

## Constraints

- Every language `wiki-code` supports (Rust, Python, C/C++, Go, JS/TS incl. Vue/Svelte scripts, bash) at full depth for flowcharts; shapes not handled yet get an explicit "not covered" note. Rust, Python and C/C++ are charted today (a preview); the others are planned.
- Research before extending; no tuning to one author's tree.
- Text in English.

## Open

See [open-questions.md](open-questions.md).
