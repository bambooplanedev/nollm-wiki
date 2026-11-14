# plaz — design

Approved 2026-11-13.

Status: function charts for Rust and Python, call targets, `plaz build` and `plaz setup` exist (a preview, built from source). State diagrams, "what changed in the logic", the other languages and publishing are planned; each is marked as planned where it appears below.

## Why

Large projects written mostly by coding agents are too big to hold in one head, and the owner did not write the code. Bugs surface late, at manual checks, and that is where technical debt grows. The worst ones are not broken wiring but wrong logic: an action runs "because X", and X is wrong, incomplete, or only valid under other conditions, yet the action runs every time.

plaz shows what the code really does, so that:

- the owner reads each decision with its literal condition in a browser and sees "that is not what I meant";
- an agent, after its change, sees which conditions and transitions changed (planned: see "What changed in the logic").

The map never judges whether a "because" is right. It shows it; the human judges. It shows changes; it does not enforce acknowledgement.

## A separate tool from kalka

kalka is for agents: its measure is whether an agent finds exact `path:start-end` context fast. plaz is for people: its measure is whether a person understands the logic correctly. Mixing them would let map changes move kalka's measured agent behaviour, and one README would promise two products.

plaz depends on `wiki-code` only: a crate in this workspace, used by path, that holds the code model (file walking, parsing, the item model, call sites, the candidate index used to find call targets, comment text). kalka's pages, search and MCP tools live in the `kalka` crate, which plaz does not use, and kalka knows nothing about the map. One set of tree-sitter grammar versions, pinned exactly in `wiki-code`, serves both. A compiled wiki is optional: the map is built from sources alone. When a compiled wiki vault is present, references to code become wikilinks to its code pages; without it they are plain `path:start-end`. Navigation between charts uses the map's own pages and never needs the wiki.

## What it shows

No end-to-end flows, no entry points. Each function gets a section with:

1. **Called from**: the functions that call it, each linking to its chart, split into "certainly calls" and "may call (if this `get` is this one)".
2. **Decision flowchart** (UML activity, Mermaid) of the body: every `if` / `else if` / `switch` / `match` arm is a diamond holding the literal condition code and the whole comment attached to it; actions and calls in order; call nodes are clickable where the renderer allows.
3. **Calls**: the callees as a plain wikilink list (guaranteed navigation in Obsidian and Quartz).

The person builds the path by moving up and down between charts. The map makes no claim it cannot back.

**State diagrams** (planned, not built), simple shape only: an enum-typed field assigned enum literals, with the guard from the enclosing branch and the method or arm as the event. Shown as a *candidate* state machine. Other shapes (computed targets, string-literal unions, typestate) get a "not covered" note. The owner states that the order state machine of a C++ trading system is implemented fully in this shape (not checked yet).

## Call targets: four forms, never a guess

The code is deterministic, so an undetermined target is written as what it is: "either … or …".

| Form | When | Shown as |
|---|---|---|
| **Resolved** | qualified path that resolves, `self`/`Self` receiver with no override in a subclass, or a receiver of known type, with one target | a link |
| **One of** | member call on a receiver of unknown type with project candidates, or `self` call with overrides | "one of: A, B, or outside the project", each a link |
| **Outside** | path or receiver type known to be external (std, dependency, an import that resolves to no project file) | "outside the project" |
| **Hidden** | call to a parameter, local, closure, function value, unknown `Q::` path, nested def | "target not visible", never "outside", never "one of" |

The node text is always the literal call (`ask(messages)`); the form only decides the link. When unsure: Hidden.

Each chart carries a coverage line: counts of resolved / one of / outside / hidden calls, plus branches not drawn as diamonds (Rust `?`, `unwrap_or`, conditions inside closures or ternaries).

## What changed in the logic (planned, not built)

Against any commit, default HEAD (uncommitted work vs last commit). plaz would extract the old tree from a `git archive` of that commit into a temporary directory and compare by node identity = function + condition text + arm order (set difference, no tree matching). The output would be a page for people and the same text for agents through the CLI. Git is the history; no map history is stored.

## Output

Markdown pages with Mermaid, written by default into `<source>/.kulman/map/`, beside kalka's pages, or into a vault given with `--vault`, with references to code as wikilinks to the wiki's code pages when that vault has them and as plain `path:start-end` otherwise. Quartz builds them into a browser site (the main human view); Obsidian opens them as a vault; agents read the same pages or call the CLI. plaz emits Quartz-compatible content; it does not ship or run Quartz. One vault holds the map of one source tree: a build that finds no code file warns and leaves the vault as it is.

## Non-goals

- No model, no paraphrase of conditions, no guessing of intent, no flagging of "suspicious" logic.
- No end-to-end flows, entry points, or sequence diagrams (navigation between charts replaces them).
- No commit/PR diff output (skipped for now).
- No stored map history.
- No project specs as a source: the map must work on any repo.
- No precise name resolution of our own (Sourcetrail and GitHub stack-graphs both died on per-language upkeep).
- No cross-language links.
- No changes to kalka's behaviour, search, hints or MCP tools.

## Anti-goal

A misleading map: a person reads it and comes away with an understanding that does not match what the project does. If the map cannot be sure, it says so in place; an honest "not visible" beats a confident wrong link.

## Decisions

Decided by the owner:
- Audience: the owner first, the agent second.
- Literal code and comments only, no model; doubt shown as "either … or …".
- Logic inside functions plus caller/callee navigation.
- State diagrams in the simple shape; sequence diagrams deferred.
- Browser main view (Quartz); Obsidian and agent access too.
- No stored history; compare with commits on demand.
- A tool of its own, depending on kalka's code layer (`wiki-code`) as a library; it began in a repository of its own and is now a crate of this workspace, depending on it by path.

Made during design (open to change):
- Change baseline HEAD by default.
- Comments shown whole up to a line cap, then a link to the lines; never cut to the first sentence.
- Charts always complete; no collapsing of large functions.
- Python nested defs and closures drawn as their own sub-charts.
- Mermaid output escaped (quotes, `{}`, `<>`, `|`) and validated with mermaid-cli in tests; clicks checked in Quartz and Obsidian, with the wikilink list as the fallback.
- Measurement gate before release, on projects not used to design this: zero wrong "resolved" links, every "one of" contains the true target or is "hidden", every diamond's text equals the source.

## Constraints

- Every language `wiki-code` supports (Rust, Python, C++, Go, JS/TS incl. Vue/Svelte scripts, bash) at full depth for flowcharts; shapes not handled yet get an explicit "not covered" note. Rust and Python are charted today (a preview); the others are planned.
- Research before extending; no tuning to one author's tree.
- Text in English.

## Open

See [open-questions.md](open-questions.md).
