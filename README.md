**English** · [Українська](README.uk.md)

<div align="center">

# nollm-wiki

**A map of your project for coding agents: where each function lives, what calls it, and which lines to read, compiled without an LLM.**

[![CI](https://github.com/bambooplanedev/llm-wiki-nollm/actions/workflows/rust.yml/badge.svg?branch=main)](https://github.com/bambooplanedev/llm-wiki-nollm/actions/workflows/rust.yml)
[![Release](https://img.shields.io/github/v/release/bambooplanedev/llm-wiki-nollm)](https://github.com/bambooplanedev/llm-wiki-nollm/releases)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange?logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

</div>

`wiki` compiles a project — code, notes, docs — into a cross-linked Markdown wiki and a symbol map
with line ranges, then serves it to agents over MCP. An agent asks `search` where something is and
gets `path:start-end` lines back, asks `callers` who calls a function, and reads mostly those lines
instead of grepping and opening whole files. No model is called at compile time: the same input
gives byte-identical output on any machine.

Not a semantic search engine: matching is lexical, by names, words and imports. Not a language
server: calls are resolved by name and argument count, without types.

```
$ wiki callers calls.rs resolve_calls --dir .wiki
callers of resolve_calls  wiki/src/calls.rs:195-262 — 1 functions in 1 files
wiki/src/lib.rs
  137-437  fn  compile_inner  calls resolve_calls at 302
```

## Contents

- [Features](#features)
- [Requirements](#requirements)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Usage](#usage) — [agents](#agents): [`wiki setup`](#wiki-setup) ·
  [Cursor](#cursor) · [Codex](#codex) · [others](#other-mcp-clients) — [MCP server](#mcp-server) ·
  [agent memory](#agent-memory) · [CLI](#cli)
- [Output](#output) — [a page](#a-page) · [artifacts](#artifacts) · [what agents read](#what-agents-read) ·
  [Obsidian and Quartz](#obsidian-and-quartz) · [page ids](#page-ids)
- [Measured](#measured)
- [Limitations](#limitations)
- [Development](#development)
- [License](#license)

## Features

- **Symbol search.** `search` ranks pages by names, words, docs and imports, and under each code
  page lists its matching functions and types as `path:start-end  name  signature — doc`.
- **Outlines.** `page` prints one line per symbol of a file with its line range, so an agent reads
  forty lines instead of four thousand.
- **Calls both ways.** `callers` lists the functions that call a function, with the lines of each
  call; `neighbors --symbol` lists everything a function calls, level by level.
- **MCP server.** `wiki serve --source .` compiles the project, recompiles it on edits and serves
  `search`, `page`, `neighbors` and `callers` to Claude Code, pi, Cursor, Codex or any MCP client.
- **One-command setup.** `wiki setup` installs the server and what tells the agent to use it, for
  Claude Code, pi, Cursor and Codex, and with `--hook` adds the matching code lines to each prompt.
- **Agent memory.** Notes an agent writes in `.agents/memory/` (or a per-user folder) become
  searchable pages that rank above code, with dates, aliases and `supersedes:`.
- **Languages.** Rust, Python, C++, Go, JavaScript, TypeScript, `.vue` and `.svelte` scripts,
  Markdown and plain text.
- **Deterministic.** Byte-identical output across runs, machines and thread counts; incremental
  recompiles.

## Requirements

- macOS (Apple silicon or Intel) or Linux x86_64 for the release binary. The Linux one is built on
  GitHub's `ubuntu-latest`, so an older distribution's glibc may need a build from source.
- Or Rust 1.88 or later to build from source. Windows is not tested.
- For agent use: an MCP client — Claude Code, pi, Cursor, Codex, Claude Desktop or another. `wiki setup --scope local`
  for Claude Code also needs the `claude` CLI on `PATH`.
- For local agent memory: `git`, in a repository with at least one commit.

## Installation

From a [release](https://github.com/bambooplanedev/llm-wiki-nollm/releases): download the tarball
for your platform (`aarch64-apple-darwin`, `x86_64-apple-darwin` or `x86_64-unknown-linux-gnu`) and
put the `wiki` binary in a directory on your `PATH`, since agents start it as plain `wiki`:

    curl -L https://github.com/bambooplanedev/llm-wiki-nollm/releases/download/v0.4.0/wiki-v0.4.0-aarch64-apple-darwin.tar.gz | tar -xz
    sudo mv wiki /usr/local/bin/

From source:

    git clone https://github.com/bambooplanedev/llm-wiki-nollm
    cd llm-wiki-nollm
    cargo install --path wiki      # into ~/.cargo/bin

## Quick start

In the project you want agents to navigate:

    cd your-project
    wiki setup --scope local      # compiles .wiki, installs the MCP server and a skill for this checkout

Then start Claude Code or pi in that directory: the `wiki` tools are there, and the server keeps
`.wiki/` in step with your edits. `--scope repo` writes committed files for the whole team instead,
and `wiki setup --remove` undoes either. Cursor and Codex: `wiki setup --agent cursor,codex`; other
clients: see [Agents](#agents).

To try the compiler on its own, on a generated corpus:

    wiki generate demo_raw --files 12 --seed 42
    wiki compile demo_raw demo_wiki
    wiki search gradient --dir demo_wiki
    wiki neighbors gradient_descent --max-tokens 800 --dir demo_wiki

```
$ wiki compile demo_raw demo_wiki
Compiled 12 pages (12 written) -> demo_wiki
Lint: 0 broken links, 1 orphans, 0 large

$ wiki search gradient --dir demo_wiki
gradient_descent  gradient_descent.txt  Gradient Descent — Gradient Descent is often paired with Learning Rate Schedule in production pipelines.
    Gradient Descent is often paired with Learning Rate Schedule in prod…
layer_normalization  layer_normalization.txt  Layer Normalization — Layer Normalization builds directly on the ideas behind Gradient Descent.
    Layer Normalization builds directly on the ideas behind Gradient Descent. When debugging Layer Normalization, engineers trac…
learning_rate_schedule  learning_rate_schedule.txt  Learning Rate Schedule — Learning Rate Schedule builds directly on the ideas behind Backpropagation.
    …tion Mechanism. Learning Rate Schedule is often paired with Gradient Descent in production pipelines. Numbers here are approxim…
```

On code the same search names symbols with line ranges. Against this repository:

```
$ wiki search "call edges" --dir .wiki --limit 1
calls  wiki/src/calls.rs  Calls — Call edges between code items: matched by name at compile time (`resolve_calls`), followed at query time (`Wiki::calls`).
  wiki/src/calls.rs:284-308  callers_index  pub(crate) fn callers_index<'a>(code: impl Iterator<Item = (&'a str, &'a [CodeItem])>) -> BTreeMap<String, Vec<Caller>>  — Every function's callers, keyed by its `path:start-end`: the `calls` edges turned around, each list tests last, then by…
  wiki/src/calls.rs:341-479  Wiki::call_walk  fn call_walk(&self, id: &str, symbol: &str, depth: Option<usize>, max_tokens: Option<usize>, up: bool) -> Result<String, String>  — `calls`, or with `up` `callers`.
  wiki/src/calls.rs:22-31  family  fn family(e: &Entity) -> &str  — The languages whose code calls each other: a page's code language, with every JavaScript dialect and component format a…
```

## Usage

### Agents

The workflow an agent follows is search → page → read lines: `search` finds the page and its
symbols, `page` lists a file's symbols with line ranges, and the agent reads just those lines with
its own file tool. `callers` answers "what breaks if this changes"; `neighbors` is for when the
surrounding pages matter.

| agent | `wiki setup` writes | what tells it to use the wiki |
|---|---|---|
| Claude Code | `.mcp.json` (repo) or `claude mcp add-json` (local), the `wiki-memory` skill | the server's instructions; tools load up front |
| pi | `.pi/mcp.json`, the `wiki-navigation` skill | the skill |
| Cursor | `.cursor/mcp.json` | the tool descriptions (enough in our runs) |
| Codex | `codex mcp add` (global), the [navigation rules](#navigation-rules) in `AGENTS.md` | `AGENTS.md` |
| any other MCP client | — ([by hand](#other-mcp-clients)) | the navigation rules in its instructions file |

#### `wiki setup`

`wiki setup` makes the agents use the wiki on their own. Run it in the project root:

    wiki setup [--agent claude,pi,cursor,codex] [--scope repo|local] [--hook] [--remove]

It compiles `.` into `.wiki`, then installs the MCP server (`wiki serve --source .`) and what tells
each agent to use it. pi gets the `wiki-navigation` skill (how to find code, and the memory rules).
Claude Code gets only a `wiki-memory` skill with the memory rules: it shows the server's
instructions, which carry the same navigation steps, and the server marks its tools to load up
front instead of behind a `ToolSearch` call. Codex reads neither, so it gets the navigation rules
in `AGENTS.md`, between `<!-- wiki:navigation -->` markers. Agents are detected (`.claude/`,
`CLAUDE.md` or `claude` on `PATH`; `.pi/` or `pi` on `PATH`; a `.cursor/` folder) unless `--agent`
names them. Codex is set up only when named: its MCP config is global.

- `--scope repo` — committed files for the whole team: `.mcp.json`, `.pi/mcp.json`,
  `.cursor/mcp.json`, `AGENTS.md`, `.claude/skills/wiki-memory/SKILL.md`,
  `.agents/skills/wiki-navigation/SKILL.md`, and `.wiki/` in `.gitignore`.
- `--scope local` — this checkout only: Claude Code's server goes through
  `claude mcp add-json wiki … --scope local`, the other files are written and the ones setup
  created are listed in `.git/info/exclude`. A file git already tracks is skipped; use
  `--scope repo` for it.
- Either scope adds Codex's server with `codex mcp add wiki -- wiki serve --source .`, which writes
  `~/.codex/config.toml`: Codex has no per-project MCP config, and it starts the server in the
  directory it runs in, so the one entry serves every project.
- Without `--scope`, setup asks on a terminal and fails without one.
- `--hook` — also adds the wiki's code lines for each prompt before the model runs. Claude Code gets
  a `UserPromptSubmit` entry (`wiki hint --claude-hook`) in `.claude/settings.json`
  (`settings.local.json` with `--scope local`); pi gets `.pi/extensions/wiki-hint/index.ts`. The
  hook is silent when there is no wiki, it cannot be read or nothing matches, and never blocks a
  prompt.
- `--remove` deletes exactly what setup wrote, hooks included even without `--hook`, and Codex's
  server only when it is the one setup added. Ignore lines, `.wiki/` and memory notes stay.

Every agent runs the server as plain `wiki`, so `wiki` must be on `PATH`; setup warns when it is
not. Safe to run twice. An entry, file or block at the same place with other content is left alone
and reported; setup does the rest and exits 1. A JSON config setup edits is rewritten as 2-space
pretty JSON with its keys and their order kept.

After setup: pi loads project files only after you trust the project (`/trust`, or `--approve` in
print mode), and Cursor loads a new server only after you approve it (its MCP settings, or
`cursor-agent mcp enable wiki`).

#### Claude Code by hand

    claude mcp add-json wiki '{"command":"wiki","args":["serve","--source","."]}' --scope local

or the `mcpServers` entry of [MCP server](#mcp-server) in the project's `.mcp.json`. Claude Code
shows the server's instructions, which carry the navigation steps, so no skill is needed for
finding code; the memory rules are in [Agent memory](#agent-memory).

#### pi by hand

`.pi/mcp.json` in the project:

```json
{ "mcpServers": { "wiki": { "command": "wiki", "args": ["serve", "--source", "."], "exposure": "direct" } } }
```

pi does not show MCP server instructions for `direct` servers, so it needs the navigation steps as
a skill: put the [navigation rules](#navigation-rules) in
`.agents/skills/wiki-navigation/SKILL.md`. pi loads both only after `/trust`.

#### Cursor

`.cursor/mcp.json` in the project (or `~/.cursor/mcp.json` for every project) takes the same entry:

```json
{ "mcpServers": { "wiki": { "command": "wiki", "args": ["serve", "--source", "."] } } }
```

Cursor starts the server in the project directory, so `--source .` works. A new server stays off
until you approve it, and Cursor drops that approval whenever the entry's command changes; the
server then silently does not load. In our runs the agent called `search` and `callers` with no
extra instructions.

`cursor-agent -p` (non-interactive) rejects every MCP tool call unless it runs with `--force`;
`--mode ask --force` lets the wiki tools run and still allows no edits. Checked with
`cursor-agent` 2026.10.22; the Cursor editor itself is not checked.

#### Codex

By hand, `codex mcp add wiki -- wiki serve --source .` writes this to `~/.codex/config.toml`:

```toml
[mcp_servers.wiki]
command = "wiki"
args = ["serve", "--source", "."]
```

The wiki's tools are marked read-only, so Codex runs them without asking, `codex exec` included.
Codex does not reach for them on its own, though: it needs the
[navigation rules](#navigation-rules) in the project's `AGENTS.md`. On three questions about this
repository (codex-cli 0.164.0, read-only sandbox), without them it made no wiki call and named the
wrong caller once; with them it used `search`, `page` and `callers` and answered all three.

#### Other MCP clients

Add the `mcpServers` entry of [MCP server](#mcp-server) to the client's config. A client that does
not start in the project directory, such as Claude Desktop, needs the project's absolute path in
`--source`. Put the navigation rules in its instructions file.

#### Navigation rules

What `wiki setup` puts in `AGENTS.md` for Codex, for any agent that needs them in its instructions
file:

```
## Find code with the project wiki

1. Call the wiki `search` tool with the words of what you are looking for and `kind: "code"`.
   Each symbol line is `path:start-end  name  signature`.
2. Read only those lines with your file tool. The start and end lines are exact — do not
   re-derive them with grep.
3. For a module's map, call `page` with the path; it lists every symbol with its lines. To follow
   a function through the code, call `neighbors` with the path and `symbol`. To see what calls a
   function, call `callers` with the path and `symbol` instead of grepping for its name.
4. Only if search finds nothing relevant, try other words, then fall back to grep.
```

### MCP server

`wiki serve` exposes a compiled wiki to MCP clients over stdio. `wiki setup` writes this
configuration for you; by hand it is:

```json
{
  "mcpServers": {
    "wiki": { "command": "wiki", "args": ["serve", "--source", "."] }
  }
}
```

`--source .` works for clients that start in the project directory, as Claude Code and pi do; for one
that does not, such as Claude Desktop, give the project's absolute path.

    wiki serve [--source <project>] [--dir <out>] [--tools <list>] [--memory <none|DIR>]

- `--source <project>` — also compile this project and keep the wiki in step with it.
- `--dir <out>` (`<project>/.wiki` with `--source`, else `out`) — the compiled wiki to serve.
- `--tools <list>` (`search,page,neighbors,callers`) — the tools to expose, comma-separated, from
  `search`, `page`, `neighbors`, `callers`, `lint`.
- `--memory <none|DIR>` — with `--source`, `none` leaves out this repo's local agent memory folder
  and `DIR` replaces it; a `DIR` that does not exist at start is not watched.

| Tool | Parameters | Returns |
|---|---|---|
| `search` | `query` (empty with `tag` lists the tagged pages), `tag?`, `kind?` (`code`, `docs`, `text`, `markdown`, `code:<lang>`), `limit?` (5 with a query; none for a tag-only listing), `format?` (`lines`, default, or `json`) | The `lines` text described under [search](#search), or `[{id, title, summary, updated, score, snippet, path, symbols}]` with `json` |
| `page` | `id` (id, title, or path), `full?` (false), `max_tokens?` (4000 for the outline) | The outline, or the whole page |
| `neighbors` | `id`, `depth?` (1), `max_tokens?` (4000), `max_nodes?`, `full?`, `kind?`, `symbol?` | The context pack, or with `symbol` the functions it calls |
| `callers` | `id`, `symbol`, `depth?` (1), `max_tokens?` (4000) | The functions that call `symbol`, with the lines of each call |
| `lint` (with `--tools`) | — | JSON `{total_pages, broken_links, orphans, large}` |

The tools mirror the CLI with smaller defaults: `max_tokens` is 4000 where the CLI is unbounded, and
`search` returns 5 hits where the CLI returns 10, because every hit line is re-read on each later
turn. `lint` is exposed only when `--tools` names it: it serves the person keeping the wiki, and
each exposed tool costs schema tokens on every request.

**Resources:** every page is `wiki://page/<id>` (Markdown), plus `wiki://index` (`index.json`) and
`wiki://llms.txt`.

**Freshness.** With `--source` the server compiles the project itself. An existing wiki is served
at once while the compile runs in the background; with none, the first compile runs before serving
and a failure exits. It then watches the project and recompiles incrementally on edits (ignored
paths such as `target/`, build dirs and `.git` never trigger it). Each tool call waits up to 5 s
until edits made before it are compiled. If a recompile fails or is still running, the last good
snapshot keeps serving and results gain a line `wiki may be stale: <reason>`. Short-lived barrier
files, one per call, live in `<dir>/.serve/`. Without `--source`,
the server only reads the wiki: it checks `index.json`'s fingerprint before each request and
reloads after a recompile; a failed reload keeps the previous snapshot and is retried on the next
request.

### Agent memory

An agent can keep its notes as Markdown files in a folder the wiki compiles, then `search` them
before it writes. `wiki setup` creates the folder and tells the agent about it in the skill:

- **Repo scope** (`--scope repo`): `.agents/memory/` in the project. It is committed, so everyone
  with the repo sees the notes. `wiki compile` always includes it.
- **Local scope** (`--scope local`): a folder outside the checkout, in the per-user data dir:
  `%LOCALAPPDATA%\wiki\memory\<key>` on Windows, `~/Library/Application Support/wiki/memory/<key>`
  on macOS, `$XDG_DATA_HOME` (else `~/.local/share`)`/wiki/memory/<key>` elsewhere. `<key>` is the
  repo's root commit, so every worktree and clone of one repo shares it. Nothing backs it up, and
  it survives `git clean`. A directory that is not a git repo has no local memory.

`serve --source` and `wiki setup` include the local folder. A plain `wiki compile . out` does not,
so a published site never carries private notes; pass `--memory local` for it, or `--memory DIR`
for any folder (without a resolvable one it warns and goes on). So a plain `wiki compile . .wiki`
drops local memory from `.wiki` until the server recompiles, and `wiki hint` reads `.wiki`.

Memory pages rank above code (their score is multiplied by 4), pass the `code` and `docs` filters of
`search`, and are found by the same tools as any page. When a prompt names a note, `wiki hint` adds
one `memory:` line to its code lines (see [hint](#hint)).

A date in a query finds the memory notes dated that day: `12 жовтня`, `October 12`, `12.10`,
`2026-10-12` (Ukrainian, Russian and English month names; `DD.MM` needs a two-digit day and month;
without a year every year matches). Project pages are never matched by date.

A note can retire an older one with `supersedes:` in its front matter (titles, aliases or ids, in
`aliases:` syntax). The older note then gets no boost, `search` prints ` (superseded by <id>)` after
its line (`"superseded_by"` in JSON), and `wiki hint` never shows it. Only memory notes are matched,
a name that matches nothing is ignored, and with several superseders the smallest id is shown.

Front matter on any Markdown page: `aliases:` and `tags:` are written `a, b`, `[a, b]` or a YAML
block list; tags are lowercased with any `#` stripped. The page's date is read from `updated`,
`modified` or `date`, then the same keys in a `metadata:` block one level deep, then a file name
starting `YYYY-MM-DD`. Cyrillic query words are stemmed, so another grammatical form of a word still
matches.

The skill carries these rules; copy them into the agent's instructions file if it has no skill:

```
Memory notes live in <folder> (.agents/memory, or the local folder setup named), one markdown file per fact or topic.
- Keep a note under about 16,000 characters (`wiki lint` reports `large` above that).
- Name the file for what it says. The title is the front-matter `title:`,
  else the first `# heading`, else the file name; any script works.
- A note not written in English needs English `aliases:`: agents search in English.
- Front matter, all optional: `title:`, `description:` (one line, the search
  summary), `aliases:`, `tags:`, `updated: YYYY-MM-DD` (date of the last change; other formats are
  ignored).
- A journal note is named `YYYY-MM-DD.md` and has the date in its H1.
- Mention a related note by its exact title or alias, or as `[[title]]`.
- Search before writing. Update the note that exists instead of adding a copy.
- When a decision changes, update its note. If you write a new note instead, add `supersedes: <old note's title>` to it.
- `MEMORY.md` is optional and is not searched.
```

### CLI

    wiki [--jobs N] <command>

| command | what it does |
|---|---|
| `compile` | compile a project into a wiki |
| `search` | pages and symbols for a query |
| `page` | a file's outline: one line per symbol with its line range |
| `callers` | the functions that call a function |
| `neighbors` | a budgeted context pack around a page, or with `--symbol` what a function calls |
| `lint` | broken links, orphan pages, pages too large for a pack |
| `serve` | the MCP server — see [MCP server](#mcp-server) |
| `hint` | the code lines for a task text, for prompt hooks |
| `setup` | install the server for Claude Code, pi, Cursor and Codex — see [Agents](#agents) |
| `generate` | a synthetic corpus for demos and bug reports |

`--jobs N` sets the worker threads of `compile` (Rayon's default, usually the number of CPUs); the
output is the same for any value. The query commands read `--dir <out>` (`out` by default) and never
recompile.

#### compile

    wiki compile [OPTIONS] <INPUT> <OUTPUT>

- `--incremental` — reuse `.wiki/cache.json` render fingerprints and skip unchanged pages.
- `--no-ignore` — do not apply `.gitignore`/`.ignore`/hidden-file rules while walking `<INPUT>`.
- `--emit-json` — also write `graph.json` (node/edge graph).
- `--project <NAME>` (the input directory's basename) — project name written to `index.json`,
  `llms.txt` and `AGENTS.md`. Without it, `compile .` and `compile "$PWD"` name the project
  differently.
- `--memory <local|DIR>` — also compile an agent memory folder as flagged memory pages: `local` for
  this repo's per-user folder, or a directory. `<INPUT>/.agents/memory` is always compiled.
- `--watch` — compile once, then keep watching `<INPUT>` and recompile on change. Events are
  debounced for 150 ms, events under `<OUTPUT>` are ignored, and each run reports
  `recompiled: N pages (M written)` on stderr.

If `<OUTPUT>` still holds pages written under a previous id scheme (an earlier version, or sources
renamed since), `compile` warns on stderr and leaves them in place: delete them or recompile into a
clean directory.

#### search

    wiki search [--kind K] [--tag T] [--limit N] [--format lines|json] [--dir out] [QUERY]

- `--kind <k>` — `code` (every `code:<lang>`), `docs` (`markdown` and `text`), or one exact kind:
  `text`, `markdown`, `code:<lang>` (e.g. `code:rust`). Memory notes pass both `code` and `docs`.
- `--tag <tag>` — only pages carrying this tag (`#` and case ignored; `a` also matches `a/b`). With
  no query, lists every such page. A query or `--tag` is required.
- `--limit <N>` (`10` with a query) — maximum number of hits.

The default output is one line per page, `id  path  [date  ]title — summary`. Under a code page, up
to 3 indented lines name its matching symbols, `  path:start-end  name  signature  — doc` (1-based,
inclusive range; `name` is `Owner::name` for methods). Only symbols scoring at least 30% of the best
symbol in the response are shown, and test code ranks at half score. A page without symbol lines
gets its body snippet on a four-space-indented line instead. `--format json` prints
`[{id, title, summary, updated, score, snippet, path, symbols}]`, each symbol
`{path, start_line, end_line, name, signature, doc}`.

#### page

    wiki page [--full] [--max-tokens N] [--dir out] <ID>

`<ID>` is a page id, or anything that names exactly one page: its title (any case), a slug of a
title, a source path or a unique path suffix (`query.rs`), or an absolute path under the compiled
directory. If nothing matches, the error lists the closest ids. `neighbors` and `callers` resolve
`<ID>` the same way.

```
$ wiki page calls.rs --dir .wiki
# Calls (calls)
wiki/src/calls.rs  code:rust
Call edges between code items: matched by name at compile time (`resolve_calls`), followed at query time (`Wiki::calls`).
imports:
  crate::query::Wiki
  …
15-20  fn  fn owner_key(owner: &str) -> &str  — The last segment of an owner, generics dropped: `Outer.Inner` and `a::Inner<T>` are both `Inner`.
36-48  type  struct Index<'a>  — What call sites resolve against: every function by name, every page by module name, every type name.
Index:
  51-104  method  fn new(entities: &'a BTreeMap<String, Entity>) -> Self
  …
195-262  fn  pub fn resolve_calls(entities: &mut BTreeMap<String, Entity>, graph: &Graph)  — Fills every function item's `calls` from its `call_sites`, which it empties.
```

Item lines are `start-end  kind  signature`, then `  → twin` for a paired C++ declaration or
definition and `  — doc` when there is a doc sentence. Methods sit under an `owner:` line; test
items carry `[test]`. A Markdown page lists its `headings:`, a text page only the header.

When `--max-tokens` is smaller than the outline, parts are cut in this order until it fits: test
items, then docs, then imports, then trailing items, and a last line says what went
(`… dropped: 3 test items, 5 docs — pass max_tokens: 2412 for all`). With `--full` and a budget
below the page's size, a code page prints its budgeted outline instead; any other page is cut after
whole lines.

#### callers

    wiki callers [--depth N] [--max-tokens N] [--dir out] <ID> <SYMBOL>

The functions that call function `<SYMBOL>` (`name` or `Owner::name`) on page `<ID>`, one
`start-end  kind  Owner::name` line each, grouped by file and ending `calls f at L1, L2` with the
lines that make the call; test code comes last. `--depth 2` adds the callers' callers; under
`--max-tokens` the farthest callers are cut first.

#### neighbors

    wiki neighbors [--depth N] [--max-tokens N] [--max-nodes N] [--full] [--kind K] [--symbol NAME] [--dir out] <ID>

A budgeted context pack for an agent: a breadth-first walk from `<ID>` up to `--depth` hops (1).
`--max-tokens` is a hard ceiling on the pack's estimated size (chars/4) and `--max-nodes` caps the
node count; neighbors are admitted by centrality (PageRank), and one that does not fit the remaining
budget is skipped while the walk goes on. The target comes first — a code target as its outline —
and the neighbors follow ordered by centrality, highest last, so the most important context sits
closest to the end. Without `--full`, the modules the target imports or is imported by come first,
as outlines when they fit, and the rest as one-line summaries.

`--symbol <NAME>` prints every function that function calls instead, through all levels (or
`--depth` of them), in the line format of `callers`.

#### lint

    wiki lint [--dir out]

Prints the counts of broken links, orphan pages and large pages (over 4000 tokens, which a default
`neighbors` pack would degrade to a summary; code pages are not checked), then one line per
offender. `[[links]]` in fenced blocks, inline code and the source body of a code page are not
counted. Exits 1 when any link is broken, so `lint` can gate a build.

#### hint

    wiki hint [--dir .wiki] [--claude-hook] [TEXT]

The code locations for a task text, as a prompt hook injects them: a header line, then at most 8
`path:start-end  name  signature` lines from the best-matching code pages, at most 2 per page,
without docs, tests or a line whose range holds another of the page's lines. Prints nothing and
exits 0 when the wiki is missing or nothing matches, so a hook can run it on every prompt.
`--claude-hook` reads a Claude Code `UserPromptSubmit` hook's JSON from stdin instead of `TEXT`, and
then defaults `--dir` to `$CLAUDE_PROJECT_DIR/.wiki`.

When the prompt names a memory note in its title, an alias or its summary, one
`memory: <path> — <summary>` line follows the code lines. It needs a prompt word of six or more
characters in that match, or one of four or five characters while the note scores at least twice
the best code page (before the memory boost), or a date in the prompt that is the note's `updated`
day.

#### generate

    wiki generate [--files 20] [--seed 42] <DIR>

A synthetic corpus: the same seed and `--files` always give byte-identical files.

## Output

### A page

Each source file becomes one page. The compiler owns every section but `## Notes`, which keeps what
you write there across recompiles:

```
$ wiki page gradient_descent --full --dir demo_wiki
# Gradient Descent

<!-- generated by wiki compiler — do not edit compiler-owned sections -->

## Metadata
- created: 2026-01-10
- aliases: gradient_descent, gradient_descent_notes
- kind: text
- source: gradient_descent.txt
- source_hash: 66fce8283cb6b5e45c6a3ee6b6c3e34204c9ba9c562f4d3df2dfdd7a3dd33c79

## Related
- [[learning_rate_schedule|Learning Rate Schedule]]

## Mentioned in
- [[layer_normalization|Layer Normalization]]
- [[learning_rate_schedule|Learning Rate Schedule]]

## Body
Gradient Descent is often paired with Learning Rate Schedule in production pipelines.

Revisit this after the next benchmark run.

## Notes
_(add your own notes here — preserved on recompile)_
```

A code page's body is its source; its symbols and line ranges are in `items.json` and `wiki page`.

### Artifacts

| File | What it is | For |
|---|---|---|
| `<id>.md` | one cross-linked Markdown page per source file | people and agents |
| `index.json` | the catalog: entries, links, centrality | agents and tools |
| `items.json` | each code page's items with their line ranges and calls | agents and tools |
| `index.md` | the index page | people |
| `llms.txt` | a compact index with one-line summaries | models |
| `AGENTS.md` | how an agent should use this wiki | agents |
| `graph.json` | node/edge graph, only with `--emit-json` | graph tools |
| `.wiki/cache.json` | the incremental render cache | the compiler |

### What agents read

- **`index.json`** — `{ project, root, entries: [...] }`, where `root` is the compiled directory's
  own name. Each entry has `id`, `title`, `kind`, `aliases`, `tags`, `updated` (only when the page
  has a date), `path`, `page`, `summary` (`null` when none could be derived), `degree_in`,
  `degree_out`, `pagerank`, `token_estimate`, `neighbors_out`, `neighbors_in`, `imports_out`,
  `imports_in` (the import edges among them), `defined` (top-level definition names) and `methods`.
- **`items.json`** — the code map, kept out of `index.json` so the catalog stays small. One object
  keyed by page id; each value lists the page's items with `kind`, `name`, `owner`, `signature`,
  `start_line`/`end_line`, `doc`, `public`, `twin` for a paired C++ declaration or definition, and
  for a function `calls` (the `path:start-end` of the functions it calls) and `call_lines` (for
  each, the lines that call it).
- **`llms.txt`** — pages at or above the median PageRank under `## Docs`, the rest under
  `## Optional`.
- **`AGENTS.md`** — where to start, how to fetch a neighborhood or a module's symbol map, and that
  each page's `## Notes` section is preserved across recompiles.

### Obsidian and Quartz

Cross-links are Obsidian-flavored wikilinks with an explicit target, `[[slug|Display Name]]`, and
each `<slug>.md` is a real page, so the links resolve with no plugin. Open the output directory
itself as the vault — not a parent that also holds the sources, whose file names can collide with
the pages. Quartz reads the same links: [`scripts/`](scripts/README.md) publishes a compiled wiki as
a Quartz site.

### Page ids

Page ids (and file names) match `[a-z0-9_]+`. They come from each source name with punctuation and
whitespace folded to `_`, Latin diacritics to their base letter, and Cyrillic transliterated
(`Морський маяк` becomes `morskyi_maiak`). A `mod.rs` or `__init__.py` page is named after its
directory (`tests/common/mod.rs` → `common`). Two names that fold to the same id — `Fetch (v2)` and
`Fetch v2`, two files with one stem in different directories, `orderbook.h` and `orderbook.cpp` —
are told apart by a directory segment, then a file-name discriminator, then a free id; no source
is ever dropped for a collision ([details](wiki/docs/ARCHITECTURE.md#disambiguation-ladder)). A name
with no letter or digit left after folding becomes `page_<hash>`.

## Measured

Read this first: the numbers come from agent runs on one private C++/Rust/Python/TypeScript project
of about 2,700 pages, with Claude Haiku in Claude Code, two runs per question and configuration.
They say what that model did on those questions in that project, not what the wiki does in yours,
and at two runs a difference of half a turn is noise.

`callers` and the call edges behind it, 2026-10-25. All three configurations have the wiki:

- `base` — `search`, `page` and `neighbors`, no `callers`;
- `callers` — adds `callers`;
- `+ macros` — also reads calls inside Rust macro arguments and through import aliases. This is
  v0.4.0.

"Where" questions ask where something is done (12 questions); "impact" ones what calls a function
or what a change would affect (8); the last 3 ask for callers that sit inside Rust macro arguments
(`format!`, `tokio::select!`) or behind an import alias. "Correct" counts runs; turns and input
tokens are means per run; Grep/Glob calls are totals over the runs.

| configuration | questions | correct runs | turns | input tokens | Grep/Glob calls |
|---|---|---|---|---|---|
| `base` | where | 23/24 | 5.25 | 118K | 30 |
| `callers` | where | 23/24 | 5.58 | 142K | 29 |
| `+ macros` | where | 23/24 | 5.12 | 119K | 24 |
| `base` | impact | 13/16 | 4.88 | 128K | 26 |
| `callers` | impact | 16/16 | 3.94 | 101K | 9 |
| `+ macros` | impact | 16/16 | 4.06 | 111K | 12 |
| `callers` | callers in macros or behind aliases | 6/6 | 5.83 | 140K | 8 |
| `+ macros` | callers in macros or behind aliases | 6/6 | 4.00 | 96K | 5 |

On the same project, Haiku read the line range `search` gave in 57 of 58 file reads. More, with
the cases where agents still fall back to grep: [wiki/docs/KNOWN_GAPS.md](wiki/docs/KNOWN_GAPS.md).

## Limitations

- **Matching is lexical.** No embeddings or semantic matching: "credentials" does not find `creds`.
  A text or Markdown page is linked when its name or alias appears in another page; a code page only
  by a code-shaped mention of its module (a `::` path, a `mod` declaration, its file name), an
  import, or a relative Markdown link.
- **Calls are resolved by name and argument count**, within one language, without types: a method
  called on a value of unknown type lists every same-named method the caller's files see.
- **Three formats.** Text, Markdown and code; no PDF, OCR or audio.
- **The MCP server is query-only**, over stdio; it recompiles on file edits, not on request.
- **`--watch` covers `compile` only**, and the graph export is JSON only.

Measured shortfalls and agent behaviour: [wiki/docs/KNOWN_GAPS.md](wiki/docs/KNOWN_GAPS.md).
Internals, determinism rules and how to add a language: [wiki/docs/ARCHITECTURE.md](wiki/docs/ARCHITECTURE.md).

## Development

    git clone https://github.com/bambooplanedev/llm-wiki-nollm
    cd llm-wiki-nollm/wiki

    cargo test             # unit, integration, snapshot and retrieval-eval tests
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

`wiki` is also a library; the CLI is a thin wrapper over the same API — see
[wiki/README.md](wiki/README.md). Pushing a `v*` tag builds the Linux and macOS binaries and attaches
them to a GitHub Release.

### Self-hosting

This repository compiles itself. `scripts/selfhost.sh` builds the release binary, compiles the
repository into `.wiki/` (gitignored, incremental) and runs `wiki lint` on it; a broken link fails
the script. The checked-in `.mcp.json` registers `wiki serve --source .` as the `nollm-wiki` server,
so an agent session started here can search the project; in a fresh clone, run
`scripts/selfhost.sh` once so the binary exists.

## License

[MIT](LICENSE)
