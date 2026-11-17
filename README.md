**English** · [Українська](README.uk.md)

# kulman

**kulman**, said KOOL-mahn (кульман): a design bureau's drawing board — the front door of this set:
it installs the tools and sets up a project.

[![CI](https://github.com/bambooplanedev/kulman/actions/workflows/rust.yml/badge.svg?branch=main)](https://github.com/bambooplanedev/kulman/actions/workflows/rust.yml)
[![Release](https://img.shields.io/github/v/release/bambooplanedev/kulman)](https://github.com/bambooplanedev/kulman/releases)
[![Rust 1.90+](https://img.shields.io/badge/rust-1.90%2B-orange?logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

kulman is a set of tools for coding agents and the people who work with them, named after the
drawing boards and floors of an aircraft design bureau. `calque` compiles a project into a map an
agent can search — where each function lives, what calls it, which lines to read — and `plaz` draws
the real decisions of each function for people to read. Neither calls a model: the same sources give
byte-identical output, and `kulman` installs both and sets them up in a project with one command
each.

## Why

So that a person and an agent work from the same picture of the project. The person needs it to
understand the business logic, meaning what happens and under which conditions, and to give the
agent precise tasks. The agent needs it to find what it is looking for without spending tokens on
reading it does not need. `calque` and `plaz` build that picture from the code, without a language
model, to the same precision: down to the line and the condition.

`plaz` draws a decision chart for each function, with the conditions copied from the code, and
shows where each call leads. The branches are on the page, not in the reader's head. It is for the
person: its setup registers nothing for agents.

`calque` gives the agent search across the project: where things are and who calls a function.
When the agent has to find the code itself, it cut the cost of a lookup by about half for strong
models in a lab test; when the task already names the file, it saves little
([what was measured](#what-calque-saves-and-when)).

`kulman` installs and updates both with one command and verifies the release signature.

## What calque saves, and when

Each figure holds only under the conditions given with it; the method and more figures are in
[calque's README](crates/calque/README.md#measured).

**Lookups with no file named.** Nine "where is X, and what value does it use" questions on a C++
fixture written for the test, with answer files of 530–645 lines and copies padded to 1,500 and
5,000 lines. The prompts name the component ("risk engine") but no file. 135 runs per model and arm,
with and without calque, in a test harness rather than in Claude Code; cost is at list price with
prompt caching:

| model | cost per lookup | input tokens | correct, without / with calque |
|---|---|---|---|
| Claude Sonnet 5.5 | −54% | −29%, not separated from zero | 135 / 135 |
| Claude Opus 5.5 | −51% | −21%, not separated from zero | 135 / 135 |

The saving grew with file size: −38% to −40% on the unpadded files, −59% to −63% at 5,000 lines.
Without calque these models already grep and then read a range; what it saves them is turns and the
new context each turn adds, not lines read. Weaker models read whole files when they do not know
where to look: without calque 17% (Claude Haiku 5.5), 46% (deepseek-v4.1-flash) and 58%
(qwen3.8-27b) of their code reads were whole files, with it 4–10%. Their input tokens fell by 29%,
36% and 75% over the three sizes (by 4–23% on the unpadded files), and they answered no more
questions correctly.

**Real work, with the file named.** Five days of agent traces from one project: Claude Opus and
Sonnet agents working from briefs that an orchestrating agent wrote, most of the code they read
Python and shell scripts. The brief, or a document it pointed to, named the file before 90–98% of
the 2,125 code reads; the agents found the line with `grep -n` in that file and read a window: 2.0%
of the reads were whole files, the median read 31 lines. About a third of definition lookups
(31.6–38.6%) went through calque. What its ranges could still have saved is about 0.25–0.5% of
input tokens. So in this mode calque saves little: the file is already known, a grep in it costs no more than a
calque answer, and 30% of the reads were shell code outside functions, where calque gives no range.
No saving on real work has been measured. Reliability on the same traces: 1 error in 260 calls, a
median answer time of 10–12 ms, and none of the 17 reads that started at a returned line found the
code gone.

**The prompt hint.** On ordinary prompts from two writers (1,514 and 136 prompts) the hint fired on
67% and 49% of them, and in 77 hints shown during real work it once plausibly led the agent to a
file it did not already have. So setup adds it only with `--hook`. Two gates now cut firing to 36%
and 27%; chat-like prompts still get a hint about a third of the time (37% and 27%).

**When it helps:**

- It helps when the agent has to find where code is: no file named, large files, weaker models.
  Strong models save turns rather than lines read.
- It helps little when tasks arrive with the file already named.
- It does nothing for literal text, regular expressions, every occurrence of a string, or counts:
  that stays grep's job.
- It does not cover shell code outside functions, or scripts without an extension for interpreters
  other than `sh` and `bash`.

**Limits of the evidence.** The lookup set is self-written and synthetic, with padded files and
prompts that name the component, and it ran in a test harness, not in Claude Code. The real-work
traces come from one project. No saving on real work has been shown.

## The names

| Name | Say it | The word |
|---|---|---|
| kulman | KOOL-mahn (кульман) | a design bureau's drawing board — the front door: installs the tools, sets up a project |
| calque | kahlk (калька) | a drafter's tracing paper — an exact tracing of your project that agents read |
| plaz | plahz (плаз) | the loft floor where a bureau draws the airframe's real lines full size — the real decisions of each function |
| techlog | TEK-log | an aircraft's technical log — the agent's memory of the project |
| wiki-code | — | plain name: the shared library that walks, parses and indexes code |

## Install

One line fetches `kulman` itself:

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/kulman/releases/latest/download/install.sh | sh

The script downloads the `kulman` archive for your system, checks it against the release's
`SHA256SUMS` (it stops if neither `sha256sum` nor `shasum` is there) and puts the binary at
`~/.kulman-home/bin/kulman`. It writes nothing else and edits no shell profile. Add
`~/.kulman-home/bin` to your `PATH` yourself if you want to type `kulman` in a terminal; agents
find `calque` without it, and when `~/.local/bin` is already on your `PATH`, `kulman install` links
the tools there. Releases are signed: `install.sh` checks the signature of `SHA256SUMS` when `minisign` is installed, and `kulman`
checks every release it reads (see [Checking a release](#checking-a-release)).

Then install the modules:

    kulman install

This puts `calque` and `plaz` (the release's default modules) into `~/.kulman-home/bin`, next to
`kulman`, with the release's `manifest.json` and a `state.json` in `~/.kulman-home`. Nothing is
written outside that folder. Then, in each project:

    cd your-project
    kulman setup --scope repo

This runs `calque setup` and `plaz setup` there. They write into the project's `.kulman/` folder
(the wiki, and the map under `.kulman/map/`), add the agent configs for the
agents they find (the prompt hook only with `--hook`), create `.agents/memory/` for agent notes, and add `.kulman/` to `.gitignore`.
`--scope repo` writes committed files for the whole team. `--scope local` writes its files into this
checkout only and lists the ones it created in `.git/info/exclude` instead; the exception is Claude
Code's calque server, which goes through `claude mcp add-json … --scope local` into Claude Code's
own configuration and needs the `claude` CLI. Codex's server goes into the project's own
`.codex/config.toml` with either scope, and Codex loads it only in a project it trusts.

Later, `kulman update` brings everything to the latest release, `kulman setup --scope repo --remove`
undoes a project's setup (with the scope it was set up with) and `kulman remove` deletes what kulman
installed. Commands, flags, exit codes and the layout of a release:
[crates/kulman/README.md](crates/kulman/README.md).

### Let your agent do it

If you would rather not run these steps by hand, paste this into your coding agent:

    Install or update kulman in this project by following
    https://raw.githubusercontent.com/bambooplanedev/kulman/main/docs/agent-setup.md
    Ask me before installing, updating, or changing any file outside this agent's own configuration.

Read [docs/agent-setup.md](docs/agent-setup.md) first to see what the agent will be told.

### Checking a release

A release's `manifest.json` and `SHA256SUMS` are signed with minisign by the release key,
[`release/kulman.pub`](release/kulman.pub); each archive is checked against the SHA-256 sums in
them. `kulman` refuses a release whose signature does not verify, or is missing over `https://`;
`install.sh` checks the signature when `minisign` is installed and otherwise says it did not. The
key, the commands to check a release by hand, how a release is signed and the limits:
[Signatures](crates/kulman/README.md#signatures).

## Modules

**[kulman](crates/kulman/README.md)** installs, updates and removes the modules and runs their
setup in a project. It is the installer, not a module.

**[calque](crates/calque/README.md)** compiles a project — code, notes, docs — into a cross-linked
Markdown wiki and a symbol map with line ranges, and serves it to agents over MCP: `search` answers
with `path:start-end` lines, `callers` says who calls a function. `calque setup` wires it into
Claude Code, pi, Cursor and Codex. Its README holds the CLI, the MCP server, agent memory, the
output format and the measurements.

**[plaz](crates/plaz/README.md)** writes, for every function, a Markdown page with a decision
flowchart: the literal condition of each branch, who calls the function and what each call reaches.
Rust, Python, C and C++ are charted; other languages get a "not covered yet" note. The map is for
people: `plaz setup` tells agents nothing about it.

**[techlog](crates/techlog/README.md)** is a library: where agent memory lives — the project's
`.agents/memory/` and the folders agents keep outside it — and who last changed each committed note.
calque uses it.

**[wiki-code](crates/wiki-code/README.md)** is the library calque and plaz share: it walks a source
tree, extracts functions and types with tree-sitter, and indexes calls and imports.

## Requirements

- A release runs on macOS on Apple silicon (`aarch64-apple-darwin`) and on Linux x86_64
  (`x86_64-unknown-linux-musl`). The Linux build is static and runs on glibc and musl systems alike.
  Those are the only two builds; anything else, Intel Macs included, builds from source. Windows is
  not supported.
- `install.sh` needs `curl`, `tar`, `mktemp` and `sha256sum` or `shasum`.
- For agent use: an MCP client — Claude Code, pi, Cursor, Codex, Claude Desktop or another. See
  [calque's requirements](crates/calque/README.md#requirements).

## Building from source

Building needs Rust 1.90 or later; the tests also need `git`, `jq` and a
non-root user.

    git clone https://github.com/bambooplanedev/kulman
    cd kulman
    cargo install --path crates/calque      # into ~/.cargo/bin
    cargo install --path crates/plaz
    cargo install --path crates/kulman

    cargo test
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

`cargo install` puts the binaries in `~/.cargo/bin`. On the two release systems a `kulman` built
this way installs the modules of a published release. On any other system it cannot: there is no
build for that system, and `kulman install` stops with `no build for <os>/<arch>`. There, use the
`calque` and `plaz` built here and run their own setup in each project (`calque setup --scope repo`,
`plaz setup --scope repo`). kulman's own tests install a release, which also needs one of the two
release systems. Pushing a `v*` tag that equals the workspace version builds the macOS and Linux
archives, checks that they install, and makes a draft GitHub Release with their `SHA256SUMS`; the
owner signs the draft with `scripts/release-sign.sh` and then publishes it
([Signatures](crates/kulman/README.md#signatures)).

## License

[MIT](LICENSE)
