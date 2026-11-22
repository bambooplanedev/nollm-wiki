**English** · [Українська](README.uk.md)

# `wiki`

**A map of your project for coding agents: where each function lives, what calls it, and which lines to read, compiled without an LLM.**

`wiki` is a module of the [nollm-wiki project](../../README.md), and this crate is the `wiki` binary and
the library it wraps.

`wiki` compiles a project — code, notes, docs — into a cross-linked Markdown wiki and a symbol map
with line ranges, then serves it to agents over MCP. An agent asks `search` where something is and
gets `path:start-end` lines back, asks `callers` who calls a function, and reads mostly those lines
instead of grepping and opening whole files. No model is called at compile time: the same input
gives byte-identical output on any machine.

Not a semantic search engine: matching is lexical, by names, words and imports. Not a language
server: calls are resolved by name and argument count, without types.

In the examples, long lines are shortened with `…`, and a `└` line names the parts of the line
above it; it is not output.

```
$ wiki callers calls.rs resolve_calls --dir .nollm-wiki
callers of resolve_calls  crates/wiki/src/calls.rs:197-264 — 1 functions in 1 files
crates/wiki/src/lib.rs
  145-464  fn  compile_inner  calls resolve_calls at 329
  └ the caller's lines        └ the line of the call
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
- [Library](#library) · [Determinism](#determinism) · [Docs](#docs)
- [Development](#development)
- [License](#license)

## Features

- **Symbol search.** `search` ranks pages by names, words, docs and imports, a query word that is a
  file name or path suffix (`strategy_config.h`, `manager/mod.rs`) lifts that file, and under each code
  page lists its matching functions and types as `path:start-end  name  signature — doc`.
- **Outlines.** `page` prints one line per symbol of a file with its line range, so an agent reads
  forty lines instead of four thousand.
- **Calls both ways.** `callers` lists the functions that call a function, with the lines of each
  call; `neighbors --symbol` (CLI; MCP with `--tools`) lists everything a function
  calls, level by level.
- **MCP server.** `wiki serve --source .` compiles the project, recompiles it on edits and serves
  `search`, `page` and `callers` (`neighbors` and `lint` with `--tools`) to Claude Code, pi, Cursor,
  Codex or any MCP client.
  Several servers on one `.nollm-wiki` are fine: one compiles, the others serve what it wrote.
- **One-command setup.** `wiki setup` installs the server and what tells the agent to use it, for
  Claude Code, pi, Cursor and Codex, and, with `--hook`, a hook that adds the matching code lines
  to each prompt.
- **Agent memory.** Notes in `.agents/memory/` and in the folders agents already write to (Claude
  Code's) become searchable pages, listed after the code hits, with dates, aliases and `supersedes:`.
- **Languages.** Rust, Python, C and C++ (`.c`, `.h`, `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`, `.hxx`; the included fragments `.inl`, `.ipp` and `.tcc` get no page), Go, JavaScript (`.js`, `.mjs`, `.cjs`), TypeScript (with JSX/TSX), `.vue` and `.svelte`
  scripts, shell (`.sh`, `.bash`, or no extension with a `sh`/`bash` shebang: functions, plus top-level UPPERCASE variables found by exact name), Markdown and plain text.
- **Deterministic.** Byte-identical output across runs, machines and thread counts; incremental
  recompiles.
- **Opens in Obsidian.** The pages link each other with Obsidian wikilinks, so the output folder is
  a vault with a working graph and backlinks; see [what does not carry over](#obsidian-and-quartz).

## Requirements

- macOS on Apple silicon (`aarch64-apple-darwin`) or Linux x86_64 (`x86_64-unknown-linux-musl`) for
  the release binary. The Linux build is static and runs on glibc and musl systems alike.
- Or Rust 1.90 or later to build from source (`cargo test` also needs `git`, `jq` and a
  non-root user). Windows is not tested.
- For agent use: an MCP client — Claude Code, pi, Cursor, Codex, Claude Desktop or another. `wiki setup --scope local`
  for Claude Code also needs the `claude` CLI on `PATH`.

## Installation

A release of nollm-wiki carries `wiki` for the two systems above; on any other system, build
it from source.

### With nollm-wiki

`nollm-wiki-install` installs, updates and sets up `wiki` together with the other modules; the steps are in
the [repository README](../../README.md#install). In short, once `nollm-wiki-install` is on your machine:

    nollm-wiki-install install                     # the release's default modules, into ~/.nollm-wiki-home/bin
    cd your-project
    nollm-wiki-install setup --scope local         # runs wiki setup and plaz setup here
    nollm-wiki-install update                      # later: everything to the latest release

Adding `~/.nollm-wiki-home/bin` to your `PATH` is optional, for typing `nollm-wiki-install` in a terminal (agents
find `wiki` without it; when `~/.local/bin` is on `PATH`, install links the tools there). Everything is on by default; turn things off with flags
(`nollm-wiki-install install --modules wiki`, `nollm-wiki-install setup --no-memory`). `nollm-wiki-install setup` runs every chosen
module even when one fails, and names the ones that failed at the end. `nollm-wiki-install remove` deletes what
nollm-wiki installed (without a terminal it needs `--yes`); projects set up with nollm-wiki keep `wiki` in
their agent configs, so run `nollm-wiki-install setup --remove` with the same `--scope` in them first.
Commands, exit codes and the layout of a release: [crates/nollm-wiki-install/README.md](../nollm-wiki-install/README.md).

### From source

    git clone https://github.com/bambooplanedev/nollm-wiki
    cd nollm-wiki
    cargo install --path crates/wiki      # into ~/.cargo/bin

## Quick start

In the project you want agents to navigate:

    cd your-project
    wiki setup --scope local      # compiles .nollm-wiki, installs the MCP server and (for pi) a skill

Then start Claude Code or pi in that directory: the `wiki` tools are there, and the server keeps
`.nollm-wiki/` in step with your edits. `--scope repo` writes committed files for the whole team instead,
and `wiki setup --remove` with the same `--scope` undoes it. Cursor and Codex:
`wiki setup --agent cursor,codex`; other clients: see [Agents](#agents).

To try the compiler on its own, on a generated corpus:

    wiki generate demo_raw --files 12 --seed 42
    wiki compile demo_raw demo_wiki
    wiki search gradient --dir demo_wiki --limit 2
    wiki neighbors gradient_descent --max-tokens 800 --dir demo_wiki

```
$ wiki compile demo_raw demo_wiki
Compiled 12 pages (12 written) -> demo_wiki
Lint: 0 broken links, 1 orphans, 0 large
                      └ a page no other page links to

$ wiki search gradient --dir demo_wiki --limit 2
gradient_descent  gradient_descent.txt  Gradient Descent — Gradient Descent is often paired with…
└ id              └ path                └ title — summary
    Gradient Descent is often paired with Learning Rate Schedule in prod…
    └ the matching text
layer_normalization  layer_normalization.txt  Layer Normalization — Layer Normalization builds…
    Layer Normalization builds directly on the ideas behind Gradient Descent. When debugging…
```

On code the same search names symbols with line ranges. Against this repository:

```
$ wiki search "call edges" --dir .nollm-wiki --limit 1
calls  crates/wiki/src/calls.rs  Calls — Call edges between code items: matched by name at compile time…
└ id   └ path             └ title — summary
  crates/wiki/src/calls.rs:286-310  callers_index  pub(crate) fn callers_index<'a>(…) -> BTreeMap<…>  — Every…
  └ path:lines               └ name         └ signature                                        └ doc
  crates/wiki/src/calls.rs:343-481  Wiki::call_walk  fn call_walk(&self, …) -> Result<…>  — `calls`, or with…
  crates/wiki/src/calls.rs:197-264  resolve_calls  pub fn resolve_calls(entities: &mut BTreeMap<…>, graph: &Graph)  — Fills every…
```

## Usage

### Agents

The workflow an agent follows is search → page → read lines: `search` finds the page and its
symbols, `page` lists a file's symbols with line ranges, and the agent reads just those lines with
its own file tool. `callers` answers "what breaks if this changes". Handed a plan, spec or ledger,
the agent passes its paths in `search`'s
`docs` to get a step's line range by its label (`T5k`) or title words (`wiki search T5k --docs plan.md`
on the command line). Each path is printed once, then `  start-end  label` per hit; `load-profile` and
`load profile` match alike, after the exact matches. When the limit cuts a label repeated in one
doc under the same head (`N11: …` in a ledger), both its first and last entries show. Pass the
plan, spec and ledger together, since an ID is often a label in only one of them; an empty query
lists the first 200 labels (`limit` lists more, and a final line says how many were cut); body text
is not searched, grep stays the tool for it.
When no label holds the words in a row, a query that names several labels one after another
(`Global constraints Per-batch procedure Pins`, `T9a T9k Rows`) lists each of them in query order: a
part of two or more words or one led by an ID opens its label, a single plain word must be the whole
label, and the first line names the words that open none. Otherwise labels opened by the query's
leading ID (`11.4`, `T3a`) answer, else labels holding all its words except stop words in any order;
and when no label answers at all, the table rows holding the query's words in a row (`| W21 | D12 |`
for `W21` or `D12`), a match in the first cell first; a first line names the fallback. An empty answer says no label matched and suggests other words or grep.

| agent | `wiki setup` writes | what tells it to use the wiki |
|---|---|---|
| Claude Code | `.mcp.json` (repo) or `claude mcp add-json` (local) | the server's instructions; tools load up front |
| pi | `.pi/mcp.json`, navigation steps in `.pi/APPEND_SYSTEM.md`, the `wiki-navigation` skill | the steps and the skill (pi does not show server instructions) |
| Cursor | `.cursor/mcp.json` | the tool descriptions (enough in our runs) |
| Codex | `.codex/config.toml`, the [navigation rules](#navigation-rules) in `AGENTS.md` | `AGENTS.md` |
| any other MCP client | — ([by hand](#other-mcp-clients)) | the navigation rules in its instructions file |

#### `wiki setup`

`wiki setup` makes the agents use the wiki on their own. Run it in the project root:

    wiki setup [--agent claude,pi,cursor,codex] [--scope repo|local] [--hook | --no-hook] [--memory | --no-memory] [--remove] [--yes]

It compiles `.` into `.nollm-wiki`, then installs the MCP server (`wiki serve --source .`) and what tells
each agent to use it. pi gets short navigation steps in `.pi/APPEND_SYSTEM.md`, which pi adds to
its system prompt, and the `wiki-navigation` skill (how to find code, and the memory rules). Claude Code gets no skill: it shows the server's instructions, which carry the same
navigation steps, and the server marks its tools to load up front instead of behind a `ToolSearch`
call; its own memory is read by the wiki. Codex reads neither, so it gets the navigation rules
in `AGENTS.md`, between `<!-- wiki:navigation -->` markers. Agents are detected (`.claude/`,
`CLAUDE.md` or `claude` on `PATH`; `.pi/` or `pi` on `PATH`; a `.cursor/` or `.codex/` folder, but
not a `.codex/` that is Codex's own home) unless `--agent` names them. When none is found and
`--agent` is not given, setup asks at a terminal which to set up (numbers or names, an empty line
for none); without a terminal, or with none chosen, it still compiles the wiki and writes the
ignore lines and the memory folder, sets up no agent, says
`no agent set up: the wiki is in .nollm-wiki/; run wiki setup --agent … later` on stderr and exits 0.
`--remove` with no agent found and no `--agent` has nothing agent-specific to remove: it says
`no agent found: nothing agent-specific removed; pass --agent … to remove one agent's wiring` on
stderr and exits 0.

- `--scope repo` — committed files for the whole team: `.mcp.json`, `.pi/mcp.json`,
  `.pi/APPEND_SYSTEM.md`, `.cursor/mcp.json`, `.codex/config.toml`, `AGENTS.md`,
  `.agents/skills/wiki-navigation/SKILL.md`, and `.nollm-wiki/` in `.gitignore`. For a file it wrote
  that git ignores, setup prints `warning: <file> is ignored by git; teammates will not get it`.
- `--scope local` — this checkout only: Claude Code's server goes through
  `claude mcp add-json wiki … --scope local`, the other files are written and the ones setup
  created are listed in `.git/info/exclude`, `/.agents/memory/` included when memory is on; a
  `.codex/config.toml` that was already there is edited and not listed. A file git already tracks
  is skipped; use `--scope repo` for it.
- Agent memory is on by default: either scope creates `.agents/memory/`. `--no-memory` creates no
  `.agents/memory/`, compiles without memory and registers the server as
  `wiki serve --source . --memory none`. Running setup again with the other choice switches the
  registered server; a server added through the `claude` CLI, or a `wiki` table in
  `.codex/config.toml`, that carries settings of your own is left alone and reported, for you to
  remove and set up again.
- Either scope writes Codex's server into the project's own `.codex/config.toml`, and needs no
  `codex` CLI for it; see [Codex](#codex).
- Without `--scope`, setup asks on a terminal and fails without one. Setup asks only when stdin and
  stdout are both terminals; `--yes` is the same as having no terminal.
- The prompt hook (`--hook`, off by default) adds the wiki's code lines for each prompt before the
  model runs. Claude Code gets a `UserPromptSubmit` entry (`wiki hint --claude-hook`) in
  `.claude/settings.json` (`settings.local.json` with `--scope local`); pi gets
  `.pi/extensions/wiki-hint/index.ts`. The hook is silent when there is no wiki, it cannot be read
  or nothing matches, and never blocks a prompt. It is off by default because it still fires on
  about a third of chat prompts (see `docs/KNOWN_GAPS.md`). A run without `--hook` (`--no-hook` is
  the same) removes a hook an earlier setup wrote, matched exactly as `--remove` matches it, and
  prints `removed <file> …: the prompt hint is off by default now; run setup again with --hook to
  keep it`; a file it cannot read is left and named (`left <file> as it is: …`) without failing the
  run. Re-run setup with `--hook` to keep it: setup does not remember the choice. A run
  limited to some agents (`--agent claude`) leaves the other agents' hook as it is.
- `--remove` deletes exactly what setup wrote, hooks included, and Codex's block only when it is
  exactly the one setup wrote (the file goes with it when nothing else is left in it, and `.codex/`
  after it). Ignore lines, `.nollm-wiki/` and memory notes stay.

In a project with `CLAUDE.md` and no `AGENTS.md`, `--agent codex` adds Codex's server but skips the
navigation steps (exit 1) instead of creating `AGENTS.md`: pi reads only `AGENTS.md` when both
exist, so it would stop seeing `CLAUDE.md`. An `AGENTS.md` an earlier setup created there, holding
nothing but the steps, is removed for the same reason. To get them, rename `CLAUDE.md` to `AGENTS.md`, create a
new `CLAUDE.md` whose only line is `@AGENTS.md` so Claude Code still loads it, and run setup again;
Codex and Cursor then read the same rules. If `CLAUDE.md` is committed, that changes team files:
run setup with `--scope repo`, since local scope skips a tracked `AGENTS.md`.

pi's steps go in a marked block (`<!-- wiki:navigation -->`) in `.pi/APPEND_SYSTEM.md`; `--remove`
takes out the block, and the file when nothing else is left. pi uses a project `.pi/APPEND_SYSTEM.md`
instead of the one in its agent dir (`$PI_CODING_AGENT_DIR`, else `~/.pi/agent`), never both. So when
the agent dir has an `APPEND_SYSTEM.md` and the project has none, setup skips the steps (exit 1)
rather than hide yours in this project; copy it to `.pi/APPEND_SYSTEM.md` and run setup again to get
both. When the agent dir's `AGENTS.md` holds a `<!-- wiki:navigation -->` block, an older copy placed
by hand that pi loads in every project, setup warns; it never edits the agent dir.

A `wiki` installed by nollm-wiki (run as `~/.nollm-wiki-home/bin/wiki`, or from `$NOLLM_WIKI_HOME/bin`) has the
agents start it as `"${NOLLM_WIKI_HOME:-$HOME/.nollm-wiki-home}/bin/wiki"` through `sh -c` (the prompt hook
runs that line, pi's extension builds the same path itself), so no agent needs it on `PATH` and no
one's home path lands in a shared file; with a `NOLLM_WIKI_HOME` other than `~/.nollm-wiki-home` setup warns
that the agents need that variable too (a home path containing `"`, `$`, a backtick or `\` is not
supported by this line). An entry in either form counts as setup's own and is rewritten to the form
this `wiki` writes. Any other `wiki` (built from source, installed elsewhere) has every agent run
the server as plain `wiki`, so `wiki` must be on `PATH`; setup warns when it is not, or when the
first `wiki` on `PATH` is another program (one whose `--version` does not print `wiki <version>`
within 2 seconds): agents would start that one, so put this `wiki` earlier on `PATH`. Safe to run
twice. An entry, file or block at the same place with other content is left alone and reported;
setup does the rest and exits 1. A JSON config setup edits is rewritten as 2-space pretty JSON with
its keys and their order kept.

After setup: pi loads project files only after you trust the project (`/trust`, or `--approve` in
print mode); `cursor-agent` loads a new server only after it is enabled
(`cursor-agent mcp enable wiki`), and the Cursor editor lists it under Settings → Tools & MCP.

#### Claude Code by hand

    claude mcp add-json wiki '{"command":"wiki","args":["serve","--source","."]}' --scope local

or the `mcpServers` entry of [MCP server](#mcp-server) in the project's `.mcp.json`. Claude Code
shows the server's instructions, which carry the navigation steps, so no skill is needed for
finding code; the wiki reads Claude Code's own memory, see [Agent memory](#agent-memory).

#### pi

`wiki setup --agent pi` writes `.pi/mcp.json`, the navigation steps in `.pi/APPEND_SYSTEM.md` and
the `wiki-navigation` skill; pi loads them after `/trust` (`--approve` with `pi -p`). pi needs the skill: it does not show MCP server
instructions for `direct` servers, so all it learns about the wiki is the tool descriptions and the
skill. Without it, in our runs the model grepped, and when a user-installed extension made pi load a
skill before it answered (we ran one called skill-first), it picked an unrelated one
(`systematic-debugging` for "where is" questions).

By hand, `.pi/mcp.json` in the project:

```json
{ "mcpServers": { "wiki": { "command":"wiki", "args": ["serve", "--source", "."], "exposure": "direct" } } }
```

and the [navigation rules](#navigation-rules) in `.agents/skills/wiki-navigation/SKILL.md`, under
a front matter with `name: wiki-navigation` and a `description:` that says when to use it
(`wiki setup` writes "Find where something is or how it works in this project's code. Use before
grep, find or reading whole files, whenever the task needs a function, type, handler or module
location."); pi finds a skill by them.

Use only the project's server: if `~/.pi/agent/mcp.json` also has a `wiki` server, under any name,
remove it. With both, the model sees every wiki tool twice under two names (pi names them after the
server: `mcp__wiki__search` for this one), and the skill `wiki setup` writes names only
`mcp__wiki__*`. To try it without letting pi edit files:
`pi -p --approve --no-session --exclude-tools edit,write "where is …"`.

Measured once per question with Mellum2.1 12B-A2.5B on llama-server, on three questions about the
private project described in [Measured](#measured): all six answers were correct; with the skill
the three took 10 tool calls and 109 s, without it 19 and 160 s. With the skill, pi answered "what
calls X" with `search`, then `callers`, and did not load an unrelated skill in these runs. A
question whose words differ from the code's (`credentials` for `load_member_creds`) still ended in
grep.

#### Cursor

`.cursor/mcp.json` in the project (or `~/.cursor/mcp.json` for every project) takes the same entry:

```json
{ "mcpServers": { "wiki": { "command":"wiki", "args": ["serve", "--source", "."] } } }
```

Cursor starts the server in the project directory, so `--source .` works. In `cursor-agent`, a new
server stays off until you approve it, and `cursor-agent` drops that approval whenever the entry's
command changes; the server then silently does not load. In our runs the agent called `search` and
`callers` with no extra instructions.

`cursor-agent -p` (non-interactive) rejects every MCP tool call unless it runs with `--force`;
`--mode ask --force` lets the wiki tools run and still allows no edits. Checked with
`cursor-agent` 2026.10.22. In the Cursor editor (3.30), with the project folder opened, the
project's `.cursor/mcp.json` server appeared under Settings → Tools & MCP and worked after a
window reload, also when git ignores the file (local scope).

#### Codex

`wiki setup --agent codex` appends this to the project's own `.codex/config.toml`, and creates the
file when it is not there; with `--no-memory`, `args` also has `"--memory", "none"`:

```toml
[mcp_servers.wiki]
command = "wiki"
args = ["serve", "--source", "."]
```

The server then exists only in this project, with either `--scope`: Codex starts it where setup ran,
not in every session. Codex reads a project's `.codex/config.toml` only in a project it trusts, so
setup ends with `Codex loads the server only in a project it trusts: run codex in the project and
accept the question, or add [projects."<project path>"] trust_level = "trusted" to
~/.codex/config.toml`, with `<project path>` replaced by the project's absolute path. The `codex`
command asks the trust question itself; the Codex desktop app (the ChatGPT app) did not ask when
the project was opened, and loaded the project's server once that entry was in
`~/.codex/config.toml`. Setup itself never edits Codex's own `~/.codex/config.toml`, and does not
treat a `.codex/` that is that folder as a project. Checked with codex-cli 0.169.0-alpha.11.1 (the
version bundled in that app); which earlier version first reads the project file is not checked.

Setup appends the block only to what it can recognise. The block is its own when the header
`[mcp_servers.wiki]` is followed by exactly the `command` line and one of the two `args` lines;
setup then switches memory by rewriting only `args`, and `--remove` deletes it. Anything else that
may define the server (extra keys such as `env` or `cwd`, a quoted or dotted key, an inline
`mcp_servers = { … }`, a file reached through a link, or one git tracks in local scope) is left as
it is and reported as `skipped .codex/config.toml wiki server: …` (exit 1), because a second
definition would make the file invalid; add the server by hand then. Other tables in the file are
never changed.

When `codex` is on `PATH`, every setup and `--remove` run that can edit the project's file asks
Codex for a machine-wide `wiki` server (`codex mcp get wiki`, run outside any project, so that a
parent project's file does not answer). One that exists is another server, and it still runs in
every other Codex session: setup reports it, with the command that removes it, on the run that finds
the project without a `wiki` server of its own (exit 1); `--remove` notes it on every run, without
changing the exit code. Without `codex` on `PATH` setup does not look.

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
3. Look up names you already know with `search`; put several in one query. Use `page` only to
   see every symbol of a module you have not seen. To see what calls a function, call `callers`
   with the path and `symbol` instead of grepping for its name.
4. Do not call grep, find or ls in the same turn as `search`. Wait for the `search` answer and
   read it first; most of the time it already holds what you need.
5. Pick the tool by the question: where something is defined or handled, `search`; who calls a
   function, `callers`; every occurrence of a literal string (an env var, a message, a config key),
   grep.
6. Only if search finds nothing relevant, try other words, then fall back to grep.
```

### MCP server

`wiki serve` exposes a compiled wiki to MCP clients over stdio. `wiki setup` writes this
configuration for you; by hand it is:

```json
{
  "mcpServers": {
    "wiki": { "command":"wiki", "args": ["serve", "--source", "."] }
  }
}
```

`--source .` works for clients that start in the project directory, as Claude Code and pi do; for one
that does not, such as Claude Desktop, give the project's absolute path.

    wiki serve [--source <project>] [--dir <out>] [--tools <list>] [--memory <none|DIR>]

- `--source <project>` — also compile this project and keep the wiki in step with it.
- `--dir <out>` (`<project>/.nollm-wiki` with `--source`, else `.nollm-wiki` if that directory exists, else
  `out`) — the compiled wiki to serve.
- `--tools <list>` (`search,page,callers`) — the tools to expose, comma-separated, from
  `search`, `page`, `neighbors`, `callers`, `lint`.
- `--memory <none|DIR>` (repeatable) — with `--source`, `none` turns off all memory, the agents'
  folders and `.agents/memory` alike, and a `DIR` replaces the discovery of the agents' folders
  (`none` with a `DIR` is a usage error); a `DIR` that does
  not exist at start is picked up when it appears if its parent exists (the parent is watched
  recursively; a missing parent needs a restart). On Linux the parent watch costs one inotify watch
  per directory under it, so pass an existing folder when the parent is large. `.agents/memory` is
  compiled unless `--memory none` is given. Behaviour change: before 0.6.6 `none` turned off only
  the discovery and `.agents/memory` was still compiled.

| Tool | Parameters | Returns |
|---|---|---|
| `search` | `query` (empty with `tag` lists the tagged pages), `tag?`, `kind?` (`code` without test pages, `tests`, `docs`, `text`, `markdown`, `code:<lang>`), `docs?` (markdown paths, relative to `--source` or else the server's working directory; searches heading and bold-step labels instead, then table rows when no label matches, not with `kind` or `tag`), `limit?` (5 with a query, 10 with `docs`; none for a tag-only listing), `format?` (`lines`, default, or `json`) | The `lines` text described under [search](#search), or `[{id, title, summary, updated, score, snippet, path, symbols}]` with `json` |
| `page` | `id` (id, title, or path), `full?` (false), `max_tokens?` (4000 for the outline) | The outline, or the whole page; a memory note's page is its text |
| `neighbors` (with `--tools`) | `id`, `depth?` (1; with `symbol`, no limit), `max_tokens?` (4000), `max_nodes?`, `full?`, `kind?`, `symbol?` | The context pack, or with `symbol` the functions it calls |
| `callers` | `id`, `symbol`, `depth?` (1), `max_tokens?` (4000) | The functions that call `symbol`, with the lines of each call |
| `lint` (with `--tools`) | — | JSON `{total_pages, broken_links, orphans, large}` |

`search`'s `tag` and `format`, `page`'s `full` and `callers`' `max_tokens` are accepted but left out
of the tool schemas: agents did not pass them, and every schema field costs tokens on every request.
A `page` or `callers` result cut short by its budget names `max_tokens` itself.

The tools mirror the CLI with smaller defaults: `max_tokens` is 4000 where the CLI is unbounded, and
`search` returns 5 hits where the CLI returns 10, because every hit line is re-read on each later
turn. `lint` is exposed only when `--tools` names it: it serves the person keeping the wiki, and
each exposed tool costs schema tokens on every request. `neighbors` is opt-in too: agents almost
never called it (16 of 11,056 calls in the measurement rig, 0 of 37 on the bench) and its schema
took about a quarter of the per-turn tool tokens. `--tools search,page,neighbors,callers` brings it
back. A committed AGENTS.md block or skill that still names it is not updated by `wiki setup`, which
leaves a block that differs from its own alone and reports it: delete the block or the skill file,
then run setup again.

**Freshness.** With `--source` the server compiles the project itself. An existing wiki is served
at once while the compile runs in the background, and a call waits at most 1 s for that first
compile; with none, the first compile runs before serving and a failure exits. It then watches
the project and recompiles incrementally once edits pause for 150 ms (at most 2 s; ignored paths such as `target/`, build dirs and
`.git` never trigger it). Each tool call waits up to 5 s
until edits made before it are compiled. If a recompile fails or is still running, the last good
snapshot keeps serving and results gain a line `wiki may be stale: <reason>`. Short-lived barrier
files, one per call, live in `<dir>/.serve/`. Several servers on one `<dir>` (one per agent session) share
it: the one holding `<dir>/.serve/writer.lock` compiles, the others answer from what it writes and
one of them takes over when it exits. Without `--source`,
the server only reads the wiki: it checks `index.json`'s fingerprint before each request and
reloads after a recompile; a failed reload keeps the previous snapshot and is retried on the next
request.

**Replaced binary.** A server that sees its `wiki` executable replaced or removed (a new install,
a rebuild, a retargeted symlink) stops compiling and writing to `<dir>` for the rest of its life, so
an old compiler cannot overwrite a newer wiki. It stays up and keeps answering from `<dir>`,
reloading it as above. It says so once on stderr, and every result gains a line
`wiki may be stale: the wiki server is out of date, ...`: restart the agent session or reconnect the
MCP server.

### Agent memory

Agents keep notes as Markdown files. `wiki` compiles the folders they write to, and `search` and
`wiki hint` show the matching notes after the code. The sources:

- **`.agents/memory/`** in the project. `wiki setup` creates it in both scopes unless given `--no-memory`; in `--scope local`
  it also lists `/.agents/memory/` in `.git/info/exclude`, so the notes stay private. `wiki compile`
  includes it unless `--memory none` is given.
- **The agents' own folders.** `serve --source` and `wiki setup` look for them. Today that is Claude
  Code's `<config dir>/projects/<slug>/memory/`, where the config dir is `CLAUDE_CONFIG_DIR` or
  `~/.claude` and `<slug>` is the path of the repo's main checkout with every UTF-16 unit that is not
  an ASCII letter or digit written as `-` (so a Cyrillic or accented letter is one `-`, a character
  outside the BMP two), cut at 200 units with a hash, as Claude Code does. A
  worktree or a subdirectory therefore shares the folder of its main checkout. A `logs/` folder
  inside it is skipped, and so is `MEMORY.md`.
- **`--memory DIR`** (repeatable) for any other folder. On `serve`, `--memory DIR` replaces the
  discovery. On `compile` and `serve`, `--memory none` turns off all memory, `.agents/memory`
  included (a behaviour change: it used to keep `.agents/memory`).

A plain `wiki compile . out` compiles only `.agents/memory` and the `--memory` folders, never the
agents' folders, so a published site never carries private notes. So a plain `wiki compile . .nollm-wiki`
drops the agents' notes from `.nollm-wiki` until the server recompiles, and `wiki hint` reads `.nollm-wiki`.

Migration: `compile --memory local` is gone, and notes in the per-user folder of earlier releases
are not compiled any more. Move them to `.agents/memory`, or pass `--memory <that folder>`.

`search` lists the code and docs exactly as if no notes existed, then up to two notes, one per line,
as `memory (<origin>): <path> — <summary>`, each followed, when its body matched, by its snippet on
an indented line. A note is listed when a query word of four or more characters is in its title,
aliases or summary, or the query writes the note's `updated` day, or the note scores at least twice
the best code hit (the note is scored over project pages plus notes, the code hit over project pages
only). Notes pass the `code` and `docs` filters, and `wiki hint` adds one memory line by the same
rule (see [hint](#hint)). Notes never move a code page's rank: PageRank and the `llms.txt` median
count project pages only.

Each note row says where the note came from: `memory (local): /abs/x.md — summary` for a note in an
agent folder, a `--memory` folder or an `.agents/memory` file git does not track (or no git), and
`memory (repo, Jane Doe): .agents/memory/x.md — summary` for a committed one, with the author name
of the file's last commit (never the email; left out when git cannot tell), `, you` when that
commit's author email is your `git config user.email`, ignoring case
(`memory (repo, Jane Doe, you): …`; no git or no email: nobody is you; the email is only compared,
never stored or shown) and `, edited` when the work tree differs from that commit. JSON hits carry
`"memory": true`, `"origin": "local"` or `"repo"`, `"author"` when known, `"mine": true` for your
notes and `"edited": true` when edited. The origin is read when the wiki is compiled, by
`wiki compile` and by each recompile of `serve --source`; answering a `search` or a hook never calls
git. Notes are ordered by trust, the score order kept among equals: local notes first, then your own
or edited committed notes, then the rest; `search` lists the two most trusted notes that pass, and
the hook's one memory line is the most trusted.

A date in a query finds the memory notes dated that day: `12 жовтня`, `October 12`, `12.10`,
`2026-10-12` (Ukrainian, Russian and English month names; `DD.MM` needs a two-digit day and month;
without a year every year matches). Project pages are never matched by date.

A note can retire an older one with `supersedes:` in its front matter (titles, aliases or ids, in
`aliases:` syntax). The older note then never appears after the code hits and `wiki hint` never shows it; a tag listing prints ` (superseded by <id>)` after
its line (`"superseded_by"` in JSON). Only memory notes are matched,
a name that matches nothing is ignored, and with several superseders the smallest id is shown.
A committed note never retires an uncommitted one.

Front matter on any Markdown page: `title:` is the title (else the first `# heading`, else the file
name) and `description:` is the one-line summary that memory rows print and the gate matches on;
`aliases:` and `tags:` are written `a, b`, `[a, b]` or a YAML
block list; tags are lowercased with any `#` stripped. The page's date is read from `updated`,
`modified` or `date`, then the same keys in a `metadata:` block one level deep, then a file name
starting `YYYY-MM-DD`. Another grammatical form of a word still matches (`provisioning` finds
`brew_provision`, `сторінок` finds `Сторінка`): query and field words are compared by their Snowball
stem, English or Ukrainian by script, below an exact or prefix match in rank. A text page over about
16,000 characters (4000 tokens) is reported by `wiki lint` as `large` (see [lint](#lint)).

pi's skill carries these rules (`<memory-dir>` is `.agents/memory`); copy them into the agent's
instructions file if it has no skill:

```
Notes about this project live in `<memory-dir>`; `search` lists up to two of them after the code
hits, each on a `memory (<origin>):` line, and the prompt hook, when `wiki setup --hook` added it,
adds one.

- Write one note per fact or topic, named for what it says.
- A note not written in English needs English `aliases:` in its front matter
  (search words are English).
- A journal note is named `YYYY-MM-DD.md` and has the date in its H1.
- Search first; update the note that exists instead of adding a copy.
- When a decision changes, update its note. If you write a new note instead, add `supersedes: <old note's title>` to it.
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
output is the same for any value. The query commands read `--dir <out>` (by default `.nollm-wiki` if that directory exists, else `out`)
and never recompile.

#### compile

    wiki compile [OPTIONS] <INPUT> <OUTPUT>

- `--incremental` — reuse `<OUTPUT>/.wiki/cache.json` render fingerprints and skip unchanged pages.
- `--no-ignore` — do not apply `.gitignore`/`.ignore`/hidden-file rules while walking `<INPUT>`,
  and walk the process folders and vendored code too.

Without `--no-ignore`, a folder named `superpowers` (at any depth) and vendored code (`third_party`,
`third-party`, `vendor`, `vendored`, `node_modules`) are not walked: memory notes carry what those
process artefacts concluded, and other people's code crowds out the project's own. After a run,
`compile` prints how many files it left out, for example
`Skipped: superpowers 2280, third_party 97, ignored 16800`: one count per folder name, and `ignored`
for the `.gitignore`/`.ignore`/hidden rules. A file counts once, for its outermost skipped folder;
`.git`, `<OUTPUT>` and the memory folders are not counted, and `--no-ignore` prints no such line.
- `--emit-json` — also write `graph.json` (node/edge graph).
- `--project <NAME>` (the input path's last component; `wiki` when it has none, as for `.`) —
  project name written to `index.json`, `llms.txt` and `AGENTS.md`. Without it, `compile .` is
  named `wiki` and `compile "$PWD"` after the directory.
- `--memory <none|DIR>` (repeatable) — also compile an agent memory folder as flagged memory pages.
  `<INPUT>/.agents/memory` is compiled too; `--memory none` turns off all memory, that folder
  included (`none` with a `DIR` is a usage error); `--memory local` is an error.
- `WIKI_TIMINGS=1` (environment) — print one `timings:` line per compile to stderr: total and
  per-stage milliseconds, files read, pages written. `wiki serve --source` and `wiki setup`
  honour it too; it never changes the output.

If `<OUTPUT>` still holds pages written under a previous id scheme (an earlier version, or sources
renamed since), `compile` warns on stderr and leaves them in place: delete them or recompile into a
clean directory.

#### search

    wiki search [--kind K] [--tag T] [--docs FILE]... [--limit N] [--format lines|json] [--dir DIR] [QUERY]

- `--kind <k>` — `code` (every `code:<lang>` except test pages), `tests` (test pages only), `docs`
  (`markdown` and `text`), or one exact kind: `text`, `markdown`, `code:<lang>` (e.g. `code:rust`;
  test pages left out as with `code`). Memory notes pass both `code` and `docs`, never `tests`; up to two that pass the gate follow the hits as
  `memory (<origin>):` lines, each with its snippet line (see [Agent memory](#agent-memory)).
  A test page is a code file under a `tests` folder or named `test_*`, `*_test.*` or `*_tests.*`;
  other layouts (a `test` folder, `*.test.*` and `*.spec.*` files, a Rust `tests.rs` module, inline
  `#[cfg(test)]` code) stay under `code`. `code` and `tests` split the hits after scoring, so a page
  scores the same under either as in an unsplit search. When a test page would have been the first
  page of a `code` search, a last line says `tests also match; use kind "tests"` (not after a
  `in test pages only` line).
- `--tag <tag>` — only pages carrying this tag (`#` and case ignored; `a` also matches `a/b`). With
  no query, lists every such page. A query, `--tag` or `--docs` is required.
- `--docs <FILE>` (repeatable) — search the heading and bold-step labels of these Markdown files instead
  of the wiki (see [Agents](#agents)); paths are relative to the current directory, no compiled wiki
  is needed, and `--kind`, `--tag` and `--dir` cannot be combined with it (a usage error).
- `--limit <N>` (`10` with a query) — maximum number of hits.

The default output is one line per page, `id  path  [date  ]title — summary`. Under a code page, up
to 3 indented lines name its matching symbols, `  path:start-end  name  signature  — doc` (1-based,
inclusive range; `name` is `Owner::name` for methods). An item matches by its whole name or by one
of its `_` and case-change words (`clampQoSLevel`: `clamp`, `qo`, `s`, `level`); two adjacent words
of at most two characters also match joined (`qos`). A symbol is shown when it scores at least 30%
of the best symbol in the response, or is its page's best, so a page with any matching symbol keeps
one, and test code ranks at half score. A function whose name has exactly one other
non-test definition in another file gets a four-space-indented line under it,
`    also defined: path:start-end  name` (not for header declarations paired with their definition,
not when the name has more copies, not in `--format json`). A page without symbol lines gets its body snippet on a
four-space-indented line instead. A query word with `_` or `::` in it, or an upper-case letter after
the first (`_ack`, `Router::slice_cap_for`, `OrderBook`), is an identifier: its leading underscore
counts and its definitions are items named exactly so, in the same case (`max_weekly_fee_usd` is not `MAX_WEEKLY_FEE_USD`).
A query of identifiers alone lists each one's definitions (up to 3 and `--limit`) and nothing else,
however many sit in one file; a name that is a file's module name in any case (`auto_refill` for
`auto_refill.rs`, a folder for `mod.rs`) first lists that file as a page line. A first line
`no symbol named <name>` lists each name that nothing defines and no file is named after. When no
name of the query is defined, the ranked pages follow and the line ends
`; the pages below match its words` (`their words` for several names): they match the name's words,
not the name. With `code`, a name
defined only on test pages, or the module name of a test file only, gets
`<name>: in test pages only; use kind "tests"` first instead, followed by those test definitions
(the lines `tests` would give) and no ranked pages for it. The answer for the other names comes
after it, as if they were the whole query. In such a query a function or method line ends its signature with
`` [called by N: `callers`] ``, N being how many functions `callers` lists for it (a C++ declaration's
definition included), and has no note when N is 0. In any other query a definition the ranking missed
is added after its page's lines, or as a page after the hits; the order does not change. A Python
function defined in another function's body is a symbol for `search` only: no `page` line, no export,
no hint line, no call edge (its calls are its enclosing function's). A summary,
snippet or doc longer than 160 characters is cut with
`…`. `--format json` prints `[{id, title, summary, updated, score, snippet, path, symbols}]`, each
symbol `{path, start_line, end_line, name, signature, doc}`; a memory note adds `memory`, `origin`
and, when they apply, `author`, `mine`, `edited` and `superseded_by`.

#### page

    wiki page [--full] [--max-tokens N] [--dir DIR] <ID>

`<ID>` is a page id, or anything that names exactly one page: its title (any case), a slug of a
title, a source path or a unique path suffix (`query.rs`), or an absolute path under the compiled
directory. If nothing matches, the error lists the closest ids (pages that are not tests first) and,
when the argument's file name without extension (`thumbnail` for `thumbnail.py`) is the name of an item, up
to 3 places that define it (`defined in: app/images.py:1-3 thumbnail`, non-test files first). A path under a folder the compile
skips or a hidden folder gets a note that the folder is not compiled instead, with the `docs` search
to use for Markdown. `neighbors` and `callers` resolve `<ID>` the same way.

```
$ wiki page calls.rs --dir .nollm-wiki
# Calls (calls)
  └ title (id)
crates/wiki/src/calls.rs  code:rust
└ source           └ kind
Call edges between code items: matched by name at compile time (`resolve_calls`), followed…
imports:
  crate::formats::code::{module_stem, source_rank}
  …
15-20  fn  fn owner_key(owner: &str) -> &str  — The last segment of an owner, generics dropped…
38-50  type  struct Index<'a>  — What call sites resolve against: every function by name, every…
Index:
└ the methods of Index
  53-106  method  fn new(entities: &'a BTreeMap<String, Entity>) -> Self
  …
197-264  fn  pub fn resolve_calls(entities: &mut BTreeMap<String, Entity>, graph: &Graph)  — Fills…
└ lines  └ kind, signature                                                                 └ doc
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

    wiki callers [--depth N] [--max-tokens N] [--dir DIR] <ID> <SYMBOL>

The functions that call function `<SYMBOL>` (`name` or `Owner::name`) on page `<ID>`, one
`start-end  kind  Owner::name` line each, grouped by file and ending `calls f at L1, L2` with the
lines that make the call; test code comes last. `--depth 2` adds the callers' callers; under
`--max-tokens` the farthest callers are cut first.

#### neighbors

    wiki neighbors [--depth N] [--max-tokens N] [--max-nodes N] [--full] [--kind K] [--symbol NAME] [--dir DIR] <ID>

A budgeted context pack for an agent: a breadth-first walk from `<ID>` up to `--depth` hops (1).
`--max-tokens` is a hard ceiling on the pack's estimated size (chars/4) and `--max-nodes` caps the
node count; neighbors are admitted by centrality (PageRank), and one that does not fit the remaining
budget is skipped while the walk goes on. The target comes first — a code target as its outline —
and the neighbors follow ordered by centrality, highest last, so the most important context sits
closest to the end. Without `--full`, the modules the target imports or is imported by are admitted
first, as outlines when they fit, and the rest as one-line summaries; all of them are then printed
in the centrality order above.

`--symbol <NAME>` prints every function that function calls instead, through all levels (or
`--depth` of them), in the line format of `callers`.

#### lint

    wiki lint [--dir DIR]

Prints the counts of broken links, orphan pages and large pages (over 4000 tokens, which a default
`neighbors` pack would degrade to a summary; code pages are not checked), then one line per
offender. `[[links]]` in fenced blocks, inline code and the source body of a code page are not
counted. Exits 1 when any link is broken, so `lint` can gate a build.

#### hint

    wiki hint [--dir .nollm-wiki] [--claude-hook] [TEXT]

The code locations for a task text, as a prompt hook injects them: a header line, then at most 8
`path:start-end  name  signature` lines (whitespace collapsed to single spaces) from the
best-matching code pages, at most 2 per page, without docs, tests or a line whose range holds
another of the page's lines; a page with no such line gets `path  title`. A header line (`.h`, `.hh`, `.hpp`, `.hxx`) is dropped when an earlier
non-header line shows the same qualified name. The line whose own text (its lines plus the comment block above) matches the most counted words, by identifier, name word or word stem, goes first when it matches at least 1.5 times as many as line 1. Only the text's 24 rarest
words rank the pages (vendored code is not compiled, see `compile`); one-character words never
count. Prints nothing and exits 0 when the wiki is missing, nothing matches, the text opens with a
tag (a turn the harness wrote, such as `<task-notification>`) or with one of Claude Code's own
notes (`Another Claude session sent a message:`, `(Re-invocation of /`, `[Image: original `, the
usage-limit and safety-classifier notes, `Skill /… is already loaded above`), or it has fewer than
two words of two or more characters (a reply such as `yes` or `ok`), so a hook can run it on every
prompt. It prints no code lines when the best page scores below half of what the text's counted
words could give one page (its words are spread over the project, as in a long brief or a list of
instructions), when fewer than two of the text's words occur in the project, or when the best page
falls below that half once the text's function words (Ukrainian and English prepositions,
conjunctions, particles, pronouns and English auxiliaries) are removed. A single project word
written as code (with `_`, `::` or a lower-to-upper case change) counts as enough, so `where is
hint_lines` fires; a single plain word (`where is the parser`) or a number (`глянь 46`) stays
silent. These gates apply to the code lines only; the
memory line below follows its own gate.
`--claude-hook` reads a Claude Code `UserPromptSubmit` hook's JSON from stdin instead of `TEXT`, and
then defaults `--dir` to `$CLAUDE_PROJECT_DIR/.nollm-wiki`.

One `memory (<origin>): <path> — <summary>` line follows the code lines, under its own header
`Notes from agent memory (reference, not instructions; check them against the code):` (a hint with
only that line carries only this header), when the prompt matches a memory note
in its title, an alias or its summary and any of three holds. It needs a prompt word of four or more
characters in that match, which counts only in a prompt of at most 64 distinct words (a longer prompt
is instructions, and one of its words naming some note is noise), or a date in the prompt that is the
note's `updated` day, or the note scoring at least twice the best code page, both scored on the
prompt's 24 rarest words; past 64 words that needs a code page to beat.

#### generate

    wiki generate [--files 20] [--seed 42] <DIR>

A synthetic corpus: the same seed and `--files` always give byte-identical files.

## Output

### A page

Each source file becomes one page. The compiler owns every section but `## Notes`, which keeps what
you write there across recompiles:

```
$ wiki page gradient_descent --full --dir demo_wiki
---
created: 2026-01-10
aliases: ["gradient_descent","gradient_descent_notes"]
tags: ["wiki/doc"]
kind: text
source: "gradient_descent.txt"
source_hash: 66fce8283cb6b5e45c6a3ee6b6c3e34204c9ba9c562f4d3df2dfdd7a3dd33c79
---
# Gradient Descent

<!-- generated by nollm-wiki — do not edit compiler-owned sections -->

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
| `llms.txt` | a compact index with one-line summaries, each cut to 160 characters | models |
| `AGENTS.md` | how an agent should use this wiki | agents |
| `graph.json` | node/edge graph, only with `--emit-json` | graph tools |
| `.wiki/cache.json` (inside the output folder) | the incremental render cache | the compiler |

### What agents read

- **`index.json`** — `{ project, root, entries: [...] }`, where `root` is the compiled directory's
  own name. Each entry has `id`, `title`, `kind`, `aliases`, `tags`, `updated` (only when the page
  has a date), `path`, `page`, `summary` (`null` when none could be derived), `degree_in`,
  `degree_out`, `pagerank`, `token_estimate`, `neighbors_out`, `neighbors_in`, `imports_out`,
  `imports_in` (the import edges among them), `defined` (top-level definition names) and `methods`.
  A memory note's entry adds `memory: true`, `origin` (`local` or `repo`) and, when they apply,
  `author`, `mine`, `edited` and `superseded_by`.
- **`items.json`** — the code map, kept out of `index.json` so the catalog stays small. One object
  keyed by page id; each value lists the page's items with `kind`, `name`, `owner`, `signature`,
  `start_line`/`end_line`, `doc`, `doc_text` (the whole doc on one line, when longer than `doc`;
  scored, never shown), `public`, `test` (test code), `twin` for a paired C++ declaration or
  definition, and for a function `calls` (the `path:start-end` of the functions it calls) and
  `call_lines` (for each, the lines that call it). A shell file's calls outside any function hang
  off a `top_level` item.
- **`llms.txt`** — pages at or above the median PageRank under `## Docs`, the rest under
  `## Optional`. Each line carries the page summary cut to 160 characters.
- **`AGENTS.md`** — where to start, how to fetch a neighborhood or a module's symbol map, and that
  each page's `## Notes` section is preserved across recompiles.

### Obsidian and Quartz

The links the compiler writes are Obsidian wikilinks with an explicit target,
`[[slug|Display Name]]`, and each `<slug>.md` is a real page, so they resolve with no plugin and
Obsidian's graph view and backlinks work. Open the output directory itself as the vault — not a
parent that also holds the sources, whose file names can collide with the pages. Quartz reads the
same links: [`scripts/`](../../scripts/README.md) publishes a compiled wiki as a Quartz site.

Every page starts with YAML front matter, so Obsidian shows its properties: `created`, `updated`,
`aliases`, `tags`, `kind`, `source` and `source_hash`. `tags` begins with a kind tag, `wiki/code`,
`wiki/doc` or `wiki/memory`, then the note's own tags. A code page's `## Body` is a fenced block
tagged with the language, so comments, `#include` and `<T>` render as code. To colour the graph by
kind, add groups in Graph view > Groups with the queries `tag:#wiki/code`, `tag:#wiki/doc` and
`tag:#wiki/memory`. The compiler writes no `.obsidian/` configuration.

A note's or doc's own links are rewritten so Obsidian finds their target (code bodies are fenced
and left alone): `[[Matching Engine]]` becomes `[[matching_engine]]` when that page exists, and a
relative Markdown link to an indexed page, such as `[guide](../docs/guide.md)`, becomes
`[guide](guide.md)`. Links to anything else stay as written and show up as unresolved nodes
(in this repository, `LICENSE`). A `#` that Obsidian would read as a tag, like `#76→`, is
escaped as `\#`.

### Page ids

Page ids (and file names) match `[a-z0-9_]+`. They come from each source name with punctuation and
whitespace folded to `_`, Latin diacritics to their base letter, and Cyrillic transliterated
(`Морський маяк` becomes `morskyi_maiak`). A `mod.rs` or `__init__.py` page is named after its
directory (`tests/common/mod.rs` → `common`). Two names that fold to the same id — `Fetch (v2)` and
`Fetch v2`, two files with one stem in different directories, `orderbook.h` and `orderbook.cpp` —
are told apart by a directory segment, then a file-name discriminator, then a free id; no source
is ever dropped for a collision ([details](docs/ARCHITECTURE.md#disambiguation-ladder)). A name
with no letter or digit left after folding becomes `page_<hash>`. An id that would be `index`,
`llms`, `agents` or `graph` becomes `<id>_page`, since those names are the wiki's own files;
`compile` warns.

## Measured

The controlled measurements are synthetic: tasks written for the measurement, not real work. In the
one live project observed, briefs name the file before 90–98% of reads, and strong models read
narrowly anyway. Whether that project is typical has not been measured.

The 52 of 52 below was measured with the server's own instructions, which tell the agent to use `wiki` before grep (what `wiki setup` leaves for Claude Code), and with a paragraph in the project's `CLAUDE.md` saying to use it first, which `wiki setup` does not write. The lookup prompt said nothing about tools; the edit brief repeated the instruction. These are the authors' measurements of their own tool, on tasks and a code tree they
chose. You are welcome to join the measuring: run `wiki` on your own project and
[open an issue](https://github.com/bambooplanedev/nollm-wiki/issues) with what you see, numbers good or bad.

Every block below names its own setup, and its figures hold only there: what a model did on those
tasks in that tree, not what `wiki` does in yours. The repository README sums them up under
[What `wiki` saves, and when](../../README.md#what-wiki-saves-and-when).

Lookups with no file named, strong models, 2026-11-18. Nine "where is X, and what value does it
use" questions, written for the test, on a C++ fixture (answer files of 530–645
lines, and copies padded to 1,500 and 5,000 lines); every prompt names the component in domain words
("risk engine", "router") and none names a file. The loop is
[llm-harness-builder](https://github.com/bambooplanedev/llm-harness-builder)'s own, not Claude Code:
tools to list a directory, read a file and run bash, plus `wiki` 0.8.1's `search`, `page` and
`callers` over MCP in the `wiki` arm; prompt caching on; Sonnet and Opus with adaptive thinking at
the API's default effort, Haiku without thinking. 5 runs per task and size, 135 runs per cell, all
sizes pooled; cost is dollars at list price from the API's own token counts:

| model | arm | correct | cost per run | input tokens per run | turns |
|---|---|---|---|---|---|
| Claude Sonnet 5.5 | without `wiki` | 135/135 | $0.0213 | 16,110 | 4.2 |
| Claude Sonnet 5.5 | `wiki` | 135/135 | $0.0097 | 11,374 | 3.1 |
| Claude Opus 5.5 | without `wiki` | 135/135 | $0.0454 | 18,078 | 4.3 |
| Claude Opus 5.5 | `wiki` | 135/135 | $0.0222 | 14,330 | 3.8 |
| Claude Haiku 5.5 | without `wiki` | 132/135 | $0.0013 | 21,222 | 3.9 |
| Claude Haiku 5.5 | `wiki` | 135/135 | $0.0006 | 11,983 | 3.1 |

With `wiki` the cost per run fell by 54% (Sonnet) and 51% (Opus), with the 95% interval over the
nine tasks below zero for both; input tokens fell by 29% and 21%, with intervals that include zero.
The cost saving grew with file size: −40% / −56% / −63% for Sonnet and −38% / −54% / −59% for Opus
at the unpadded size, 1,500 and 5,000 lines. It comes from fewer turns, less new context written to
the cache each turn (Sonnet 5,219 → 1,286 tokens per run, Opus 5,092 → 1,309) and less output,
thinking included (−24% and −25%); not from fewer lines read. Without `wiki` these models grep
(2.2 times per run) and then read the hit's range: their whole-file reads (17% and 23% of code
reads) are almost all of small files, with 8 of 245 and 1 of 254 reads taking a file of more than
200 lines whole. The relative saving depends on caching: a longer shared preamble, as in a real session,
makes it smaller.

The same fixture and loop, with weaker models and an earlier version (0.6.7), 2026-11-10, 135 runs
per cell:

| model | whole-file code reads, without / with `wiki` | input tokens, all sizes | input tokens, unpadded files | correct, without / with |
|---|---|---|---|---|
| Claude Haiku 5.5 | 17% / 5% | −29% | −8% | 134 / 133 |
| deepseek-v4.1-flash | 46% / 10% | −36% | −4% | 122 / 117 |
| qwen3.8-27b | 58% / 4% | −75% | −23% | 131 / 127 |

They read whole files when they do not know where the answer is, and the ranges of `wiki` replace those
reads; most of the saving comes with the padded files, and no model answered more questions
correctly.

Real work with the file named, 2026-11-12 to 2026-11-17. Agent traces from one
C++/Rust/Python/TypeScript project of 894 pages: Claude Opus and Sonnet agents working from briefs
that an orchestrating agent wrote, with an earlier version (0.6.13) for the first half and `wiki` 0.8.1 after it; about 95% of the
code lookups were in Python and shell scripts. 7,773 tool calls, 2,125 code reads, 2,170 grep
commands, 260 `wiki` calls.

- Reliability: 1 error in 260 calls (`page` of a hook script with no extension, which 0.8.2 now
  indexes), no timeouts, median answer time 10–12 ms. None of the 17 reads that started at a line
  `wiki` returned found the code gone.
- The file was known before 90–98% of code reads, named in the brief or in a document it pointed
  to. Agents found the line with `grep -n` in that file (about 500 characters per grep, against
  818 for the median `search` answer) and read a window: 2.0% of reads took the whole file, 0.7%
  more than half of a file over 200 lines and the median read was 31 lines.
- Of the grep commands, 16% (hand-labelled sample of 330, 95% interval 12.5–20.4%) looked up the
  definition or outline of a symbol `wiki` indexes; 47% looked for literal text, regular
  expressions, counts or absence, which stay grep's work. Counting `wiki` calls against shell calls
  holding a definition grep, 31.6–38.6% of definition lookups went through `wiki` (14.5–21.5%
  through `search` alone).
- 30% of code reads were shell code outside any function, for which `wiki` has no range.
- What the ranges of `wiki` could still have saved: 0.9–1.9% of tool-result characters, about
  0.25–0.5% of input tokens once the time each read stays in context is counted. No saving on real
  work has been measured: agents that used `wiki` had longer tasks than those that did not, so the
  two cannot be compared.

Use in full Claude Code, 2026-11-18. Claude Sonnet 5.5, `wiki` 0.8.1, `claude -p` headless with the
normal login, user and project instructions, skills and the superpowers plugin loaded, and a
preamble about seven times larger than in the isolated harness (49,479 against 6,689 input tokens
per lookup run). Tools: Bash, Read, Edit, Agent, Skill and the `wiki` tools `search`, `page` and `callers`.
The tree is a C++/Rust/Python/TypeScript project snapshot. The same 13 sets of helper names in two
shapes, 2 runs per set, 26 runs per shape: a lookup ("I am working in <file>. Find where each of these helpers is defined") and a
brief that names the file too, carries the project's line about using `wiki` first and asks for one
comment above each named helper. `wiki` was the first lookup in 26 of 26 lookup runs and 26 of 26
edit runs (95% interval 87–100% each); every run succeeded. The same lookup task set in the isolated
harness (Claude Code's bare mode, 2026-11-11, an earlier build) gave 52 of 52. The difference between
the two shapes is 0 points (95% interval −13 to +13). No run delegated, and none called a skill.
Definition greps appeared in 3 of 26 lookup runs and 15 of 26 edit runs; none replaced a `wiki`
lookup, and in the edit runs nearly all came after the edit, to check the comments.

Cost in full Claude Code, 2026-11-18. The runs above, compared with a control of 26 runs per task
shape that has no `wiki`: no MCP server, no "code lookup" paragraph in the project instructions (nor
the parenthesis naming `wiki page` in the scripts folder's), and no `wiki` line in the edit brief.
Same model, tools, login, plugin and tree, a fresh copy per run; cost is dollars at list price from
the runs' own token counts, which reproduce the reported cost exactly. The with-`wiki` runs were made
earlier the same day, not interleaved with the control. 95% intervals are bootstrapped within each
set of names:

| task | `wiki` | correct | cost per successful run | turns | wall clock |
|---|---|---|---|---|---|
| lookup by name | without | 24/26 | $0.087 | 4.4 | 12.9 s |
| lookup by name | with | 26/26 | $0.064 | 2.4 | 5.1 s |
| small edit, file named | without | 26/26 | $0.128 | 9.3 | 26.1 s |
| small edit, file named | with | 26/26 | $0.147 | 12.8 | 27.1 s |

Turns are Claude Code's own count. On lookups it equals the API requests (2.4 and 4.4); on the edit
task it does not, because tool calls are issued in parallel: 5.6 requests with `wiki` against 6.6
without, one fewer.

With `wiki` the cost per successful run fell 26% on lookups (interval −37% to −16%) and rose 15%
on the edit task (+10% to +19%); per run, counting the two wrong lookups, lookups fell 20%. Of the two wrong lookups without `wiki`, one answered the ranges
without paths and one gave a range of 34 lines for a function of 11. The `wiki` tool definitions and
instructions add 1,744 tokens to every request (1,799 in the edit task, whose brief also carries a
`wiki` line). Of the 1,744, 1,449 are the tool definitions, which sit in a cache shared across runs
(the first request reads 7,195 cached tokens with `wiki` and 5,746 without, in 25 of 26 pairs; one run was a cold start); about
295 are written once per run. At the rates the runs imply that is about $0.0016 per lookup run and
$0.0024 per edit run, 2.4% and 1.6% of the run's cost, already in the figures. The first run in an
hour also writes the definitions to the cache, about $0.006 once.
On lookups the saving is fewer turns (−2.0), less cached context read again (−36,000 tokens) and
less output (−730 tokens); one search with all the names replaced about two greps and a sed. On the
edit task that overhead is about 13% of the $0.019 gap. Most of the rest goes with the first answer, an
observational split (the agent chose its first call): across 78 with-`wiki` edit runs (the 26 above,
a second run of them and the 26 below), the 37 that opened with a `page` of the whole file cost
$0.161 against $0.127 for the same task without `wiki`, +27% (interval +23% to +32%); the 41 that
opened with a `search` cost $0.131 against $0.129, +1.6% (−2% to +6%). The editing also differed: with `wiki` all
26 runs read each returned range and made one edit per helper; without it about half the runs did
one grep and one scripted edit. A second run of the with-`wiki` edit group, made later, cost $0.140, 4.6% less, which would make
the gap +9% (+5% to +13%, set-stratified, descriptive only); the edit cost is better read as +9% to +17% across the three groups of 26.
The brief's `wiki` line also tells the agent to read only the returned line
ranges, and the control removes it with the tool; a further group of 26 runs that kept `wiki` and
dropped that line cost $0.149, +17% over the same control (+13% to +21%), not less. No group read a whole
file, with or without `wiki`. Limits: targets were a ready list of exact names in Python files of
about 1,900 and 5,200 lines that the index covers; one code tree and one model; hand-written briefs;
short runs, where a long session would pay the per-run write once (about 2% of run cost here). These are
controlled tasks, not real work.

Use in live work, 2026-11-10 to 2026-11-17. The same project's traces as above, from 2026-11-10 on, in
sessions where a lead agent briefs child agents. `wiki` was the
first lookup (against a grep, find or ls in a shell call) for 29 of 83 Opus leads, 44 of 142 Opus
children and 20 of 150 Sonnet children; the figures are per agent, and the tasks of Sonnet children
differ from those of Opus children. The tools were not deferred behind a tool search and the server's
instructions were present in those sessions, so a missing tool does not explain the gap with the
controlled runs. The tools and the server's instructions were loaded. Two candidate causes were not reproduced in the controlled runs: Claude Code's larger context (a descriptive comparison, confounded with build, date and tool set) and a brief that names the file and asks for an edit (0 points, −13 to +13, at this size, with a ready list of names).
What is not known: the cause. Not yet tested: lookups done by child agents a lead spawned (none of
the 52 controlled runs delegated, while in the live work nearly all lookups were made by leads and
children), long sessions, and targets that do not arrive as a list of names. One code tree, one
team's live work, one model in the controlled runs: a larger sample from independent projects is
needed before saying how often agents use `wiki` in real work.

The prompt hint, 2026-11-17, offline over 1,514 and 136 prompts from two writers: it fired on 66.8%
and 48.5% of them. In 77 hints shown during real work it once plausibly led the agent to a file it
did not already have (95% upper bound about 9% of hints from people); the other files it named were
already in context or never opened. That is why setup adds it only with `--hook`. The two gates now cut firing to 36% and 27%;
chat-like prompts still get a hint about a third of the time (37% and 27%). The gates and what still fires are in
[docs/KNOWN_GAPS.md](docs/KNOWN_GAPS.md#agents-and-the-wiki).

Earlier runs. The `callers` block comes from agent runs on one private C++/Rust/Python/TypeScript
project of about 2,700 pages, with Claude Haiku in Claude Code, two runs per question and
configuration. It says what that model did on those questions in that project, and at two runs a
difference of half a turn is noise.

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
the cases where agents still fall back to grep: [docs/KNOWN_GAPS.md](docs/KNOWN_GAPS.md).

The prompt hook and large files, 2026-10-28. These runs use two hosted models through OpenRouter
(qwen3.8-27b and deepseek-v4.1-flash, reasoning effort low) in
[llm-harness-builder](https://github.com/bambooplanedev/llm-harness-builder), on small fixture repos
written for the purpose, 5 runs per task. Lines read are means per run.

Questions ("where is X handled, and what value does it use"), wiki and navigation rules in both
columns, without and with the hook:

| model | repo | correct, without / with hook | lines read, without / with hook |
|---|---|---|---|
| deepseek-v4.1-flash | C++, four 500-600-line files | 22/25 / 25/25 | 171 / 80 |
| deepseek-v4.1-flash | shell test harness | 10/10 / 10/10 | 310 / 230 |
| deepseek-v4.1-flash | Python and TypeScript shop | 15/15 / 15/15 | 27 / 12 |
| qwen3.8-27b | C++, four 500-600-line files | 25/25 / 25/25 | 57 / 24 |
| qwen3.8-27b | shell test harness | 10/10 / 10/10 | 284 / 59 |
| qwen3.8-27b | Python and TypeScript shop | 15/15 / 15/15 | 9 / 2 |

Edits next to look-alikes (6 tasks, 30 runs per model): passing runs without / with the hook were
30/30 / 29/30 for deepseek and 30/30 / 26/30 for qwen, whose misses were its own tool-call markup
leaking into the edit text; lines read 37 / 48 and 37 / 61, one extra small file where the hint
ranked a look-alike first.

Large files, same C++ repo, without and with the wiki: runs that read a 500-plus-line file whole
went from 14/25 to 0/25 for both models, lines read from 386 to 70 (qwen) and 551 to 184 (deepseek).

Agent memory, 2026-11-01. Notes from the agents' own folders (Claude Code's project memory,
`.agents/memory`) are listed by `search` after the code, which stays byte-identical to a run without
notes, and by the hook line. Same rig, two fixture repos with 15 synthetic notes each whose answer
is a token in one note; 30 memory tasks × 2 runs and 8 code tasks × 3 runs per arm:

| model | memory tasks right: no notes / notes after the code / 0.5.0 ranking (notes mixed in) | code tasks right |
|---|---|---|
| qwen3.8-27b + deepseek-v4.1-flash | 0/120 / 114/120 / 117/120 | 48/48 on every arm |
| claude-haiku-5-5 | 0/60 / 49/60 / 51/60 | 24/24 on every arm |

The 0.5.0 ranking put a note first in 51 of 60 code queries; the tail changes no code line. Haiku
needs the four-letter name gate: at eight letters its short, code-shaped queries never surfaced
the note in 14 of 60 runs (42/60 right). What the gate costs on code work (Haiku, 12 code tasks
with no note about them, served with 42 notes about other projects, 36 runs): a foreign note under
27 runs, none opened, 36/36 right, 74 lines read against 83 without notes, about 340 bytes more per
search answer. The hook line changes no answer on Haiku (49/60 with or without it) and saves a
search per task; on long task prompts it names a note only up to 64 distinct words (53 of 58 long
prompts showed a note before, 5 after), short questions unchanged.

## Limitations

- **Matching is lexical.** No embeddings or semantic matching: "credentials" does not find `creds`.
  A text or Markdown page is linked when its name or alias appears in another page; a code page only
  by a code-shaped mention of its module (a `::` path, a `mod` declaration, its file name), an
  import, or a relative Markdown link.
- **Calls are resolved by name and argument count**, within one language, without types: a method
  called on a value of unknown type lists every same-named method the caller's files see.
- **Three formats.** Text, Markdown and code; no PDF, OCR or audio.
- **The MCP server is query-only**, over stdio; it recompiles on file edits, not on request.
- **The graph export is JSON only.**

Measured shortfalls and agent behaviour: [docs/KNOWN_GAPS.md](docs/KNOWN_GAPS.md).
Internals, determinism rules and how to add a language: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Library

The CLI is a thin wrapper over the same API:

```rust
use std::path::Path;
use wiki::{compile, CompileOptions};
use wiki::query::{Wiki, PackBudget};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = CompileOptions { incremental: true, ..Default::default() };
    let result = compile(Path::new("demo_raw"), Path::new("demo_wiki"), &opts)?;
    println!("{} pages compiled", result.pages_total);

    let wiki = Wiki::load(Path::new("demo_wiki"))?;
    for hit in wiki.search("gradient", None, 5) {
        println!("{}\t{}", hit.id, hit.title);
    }
    let budget = PackBudget { max_tokens: Some(2000), ..Default::default() };
    if let Some(pack) = wiki.neighbors("some_id", 1, &budget) {
        println!("{}", pack.text);
    }
    Ok(())
}
```

`CompileOptions.project` (the CLI's `--project`) sets the project name written to `index.json`,
`llms.txt` and `AGENTS.md`; it defaults to the input directory's basename. Besides `search` and
`neighbors`, `Wiki` exposes `page(id)` (a rendered page's Markdown), `has_page(id)` and
`list_pages()` (`(id, title)` pairs, ascending by id).

## Determinism

Compiling the same input twice — on any machine, with any `--jobs` value — produces byte-identical
output, across the ordering of parallel work, hashing, float comparisons and path handling. The
rules that make this hold are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — pipeline internals, module map, determinism rules,
  and how to extend the compiler.
- [docs/KNOWN_GAPS.md](docs/KNOWN_GAPS.md) — where results fall short and what agents do with the
  wiki.

## Development

### Build

From the workspace root:

    cargo build --release      # target/release/wiki
    cargo test                 # unit, integration, snapshot and retrieval-eval tests; needs jq
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

### Self-hosting

This repository compiles itself. `scripts/selfhost.sh` builds the release binary, compiles the
repository into `.nollm-wiki/` (gitignored, incremental) and runs `wiki lint` on it; a broken link fails
the script. The checked-in `.mcp.json` registers `wiki serve --source .` as the `wiki` server,
so an agent session started here can search the project; in a fresh clone, run
`scripts/selfhost.sh` once so the binary exists.

## License

[MIT](../../LICENSE)
