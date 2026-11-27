**English** · [Українська](README.uk.md)

# nollm-wiki

**A project map for coding agents and decision charts for people, compiled from the code without an LLM.**

[![CI](https://github.com/bambooplanedev/nollm-wiki/actions/workflows/rust.yml/badge.svg?branch=main)](https://github.com/bambooplanedev/nollm-wiki/actions/workflows/rust.yml)
[![Release](https://img.shields.io/github/v/release/bambooplanedev/nollm-wiki)](https://github.com/bambooplanedev/nollm-wiki/releases)
[![Rust 1.91+](https://img.shields.io/badge/rust-1.91%2B-orange?logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

nollm-wiki turns a project into a map an agent can search — where each function lives, what calls
it, which lines to read — and into decision charts of each
function. Nothing calls a model: the same sources give byte-identical output.

- `wiki` compiles the map and serves it to agents over MCP;
- `plaz` draws the real decisions of each function as charts;
- `nollm-wiki-install` installs both, checks the release signature and sets them up in a project with
  one command.

## Why

So that a person and an agent can work from the same map of the project. `wiki` gives the agent
search across the project: it finds a symbol it can name without reading whole files. A file it cannot
name it finds less well: on real sessions, `search` had such a file among its first five hits in
20% of cases. `plaz` draws a decision chart for each function
for the person. Both are built from the code, without a language model. `plaz` is a preview: what
it covers and what is still open is in [its open questions](crates/plaz/docs/open-questions.md).
What was measured: [What `wiki` saves, and when](#what-wiki-saves-and-when).

## What `wiki` saves, and when

The controlled measurements are synthetic: tasks written for the measurement, not real work. In the
one live project observed, briefs name the file before 90–98% of reads, and strong models read
narrowly anyway. Whether that project is typical has not been measured.

- **Lookups with no file named** (isolated harness, strong models): about half the cost per lookup,
  −54% on Claude Sonnet 5.5 and −51% on Claude Opus 5.5 (−38% to −40% at the files' real size, more
  on files padded to 1,500 and 5,000 lines); weaker models read fewer whole files, and three of them
  (Claude Haiku 5.5, deepseek-v4.1-flash, qwen3.8-27b) gave no more correct answers. A local model,
  Bonsai 27B, gave more only with padded files: 87% against 75% over all sizes, −2 points [−13, +9]
  at the files' real size. In later runs on one C++ tree with no file named, Haiku gave 5.2 points
  more correct answers with `wiki` than without [+1.3, +10.0]; Bonsai 27B under pi gave +4 [−3,
  +10], not shown.
- **Full Claude Code, Sonnet 5.5, file named in both tasks:** lookups by name −20% per run, −26% per
  successful run (the control had 24 of 26 correct); a small edit +9% to +17% across three groups
  of 26.
- **Use:** agents used `wiki` first in 52 of 52 controlled full-Claude-Code runs (26 + 26), and in
  13% (Sonnet children) to 35% (Opus leads) of agents in the live work. The cause of that gap is not
  settled. For a local 27B model, `wiki` was the first navigation call in 100 of 100 runs on stock
  pi and in 6 of 30 under pi with workflow skills, but the tasks differ too (finding a place against
  fixing a bug in a named area).
- **It helps** when the lookup is the job: files too large to read whole, weaker models (in tokens
  read; in correct answers only Haiku, and Bonsai 27B on padded files), a list of names to find. On
  a small edit in a named file it did not help and cost 9% to 17% more, in the one task measured. It
  does nothing for literal text, regular expressions or counts: that stays grep's job.

No saving on real work has been shown. Setups, intervals and limits for every figure:
[the `wiki` module's Measured section](crates/wiki/README.md#measured). Results from other projects and
models will be added as they are measured, and you are welcome to [measure your own and open an
issue](https://github.com/bambooplanedev/nollm-wiki/issues).

## The names

| Name | Say it | The word |
|---|---|---|
| nollm-wiki | the project's name | the project: the map for agents and the decision charts for people |
| `nollm-wiki-install` | nollm-wiki install | plain name: the installer of the tools |
| `wiki` | wiki | plain name: the module that compiles the project wiki and serves it to agents |
| plaz | plahz (плаз) | the loft floor where a bureau draws the airframe's real lines full size — the real decisions of each function |
| techlog | TEK-log | an aircraft's technical log — the agent's memory of the project |
| wiki-code | — | plain name: the shared library that walks, parses and indexes code |

## Install

One line fetches `nollm-wiki-install` itself:

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/nollm-wiki/releases/latest/download/install.sh | sh

The script downloads the `nollm-wiki-install` archive for your system, checks it against the release's
`SHA256SUMS` (it stops if neither `sha256sum` nor `shasum` is there) and puts the binary at
`~/.nollm-wiki-home/bin/nollm-wiki-install`. It writes nothing else and edits no shell profile. Add
`~/.nollm-wiki-home/bin` to your `PATH` yourself if you want to type `nollm-wiki-install` in a terminal; agents
find `wiki` without it, and when `~/.local/bin` is already on your `PATH`, `nollm-wiki-install install` links
the tools there. Releases are signed: `install.sh` checks the signature of `SHA256SUMS` when `minisign` is installed, and `nollm-wiki-install`
checks every release it reads (see [Checking a release](#checking-a-release)).

Then install the modules:

    nollm-wiki-install install

This puts `wiki` and `plaz` (the release's default modules) into `~/.nollm-wiki-home/bin`, next to
`nollm-wiki-install`, with the release's `manifest.json` and a `state.json` in `~/.nollm-wiki-home`. Nothing is
written outside that folder. Then, in each project:

    cd your-project
    nollm-wiki-install setup --scope repo

This runs `wiki setup` and `plaz setup` there. They write into the project's `.nollm-wiki/` folder,
add the agent configs for the agents they find (Claude Code, pi, Cursor, Codex; the prompt hook only with `--hook`), create
`.agents/memory/` and add `.nollm-wiki/` to `.gitignore`. `--scope repo` writes committed files for the
whole team; `--scope local` writes them for this checkout only and lists them in `.git/info/exclude` (what each scope writes, and for which
agent: [the `wiki` module's setup](crates/wiki/README.md#wiki-setup)).

Later, `nollm-wiki-install update` brings everything to the latest release, `nollm-wiki-install setup --scope repo --remove`
undoes a project's setup (with the scope it was set up with) and `nollm-wiki-install remove` deletes what nollm-wiki
installed. Commands, flags, exit codes and the layout of a release:
[crates/nollm-wiki-install/README.md](crates/nollm-wiki-install/README.md).

### Let your agent do it

If you would rather not run these steps by hand, paste this into your coding agent:

    Install or update nollm-wiki in this project by following
    https://raw.githubusercontent.com/bambooplanedev/nollm-wiki/main/docs/agent-setup.md
    Ask me before installing, updating, or changing any file outside this agent's own configuration.

Read [docs/agent-setup.md](docs/agent-setup.md) first to see what the agent will be told.

### Checking a release

A release's `manifest.json` and `SHA256SUMS` are signed with minisign by the release key,
[`release/nollm-wiki.pub`](release/nollm-wiki.pub); each archive is checked against the SHA-256 sums in
them. `nollm-wiki-install` refuses a release whose signature does not verify, or is missing over `https://`;
`install.sh` checks the signature when `minisign` is installed and otherwise says it did not. The
key, the commands to check a release by hand, how a release is signed and the limits:
[Signatures](crates/nollm-wiki-install/README.md#signatures).

## Modules

**[nollm-wiki-install](crates/nollm-wiki-install/README.md)** installs, updates and removes the modules and runs their
setup in a project. It is the installer, not a module.

**[`wiki`](crates/wiki/README.md)** compiles a project — code, notes, docs — into a cross-linked
Markdown wiki and a symbol map with line ranges, and serves it to agents over MCP: `search` answers
with `path:start-end` lines, `callers` says who calls a function. Its README holds the CLI, the MCP
server, agent memory, the output format and the measurements.

**[plaz](crates/plaz/README.md)** writes, for every function, a Markdown page with a decision
flowchart: the literal condition of each branch, who calls the function and what each call reaches.
Rust, Python, C and C++ are charted; other languages get a "not covered yet" note. The map is for
people: `plaz setup` tells agents nothing about it.

**[techlog](crates/techlog/README.md)** is a library: where agent memory lives — the project's
`.agents/memory/` and the folders agents keep outside it — and who last changed each committed note.
`wiki` uses it.

**[wiki-code](crates/wiki-code/README.md)** is the library `wiki` and plaz share: it walks a source
tree, extracts functions and types with tree-sitter, and indexes calls and imports.

## Requirements

- A release runs on macOS on Apple silicon (`aarch64-apple-darwin`) and on Linux x86_64
  (`x86_64-unknown-linux-musl`). The Linux build is static and runs on glibc and musl systems alike.
  Those are the only two builds; anything else, Intel Macs included, builds from source. Windows is
  not supported.
- `install.sh` needs `curl`, `tar`, `mktemp` and `sha256sum` or `shasum`.
- For agent use: an MCP client — Claude Code, pi, Cursor, Codex, Claude Desktop or another. See
  [the `wiki` module's requirements](crates/wiki/README.md#requirements).

## Building from source

Building needs Rust 1.91 or later; the tests also need `git`, `jq` and a
non-root user.

    git clone https://github.com/bambooplanedev/nollm-wiki
    cd nollm-wiki
    cargo install --path crates/wiki      # into ~/.cargo/bin
    cargo install --path crates/plaz
    cargo install --path crates/nollm-wiki-install

    cargo test
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

`cargo install` puts the binaries in `~/.cargo/bin`. On the two release systems a `nollm-wiki-install` built
this way installs the modules of a published release. On any other system it cannot: there is no
build for that system, and `nollm-wiki-install install` stops with `no build for <os>/<arch>`. There, use the
`wiki` and `plaz` built here and run their own setup in each project (`wiki setup --scope repo`,
`plaz setup --scope repo`). The `nollm-wiki-install` tests install a release, which also needs one of the two
release systems. Pushing a `v*` tag that equals the workspace version builds the macOS and Linux
archives, checks that they install, and makes a draft GitHub Release with their `SHA256SUMS`; the
owner signs the draft with `scripts/release-sign.sh` and then publishes it
([Signatures](crates/nollm-wiki-install/README.md#signatures)).

## License

[MIT](LICENSE)
