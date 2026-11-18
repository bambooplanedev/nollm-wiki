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

`calque` gives the agent search across the project; `plaz` draws a decision chart for each function
for the person. What was measured: [What calque saves, and when](#what-calque-saves-and-when).

`kulman` installs and updates both with one command and verifies the release signature.

## What calque saves, and when

The Claude measurements are synthetic: controlled tasks written for the measurement, not real work.
The one real project where live agent work was observed is already heavily set up, with good
practice and the authors' own tooling: its briefs name the file to work in and its agents already
read narrowly. Little is left there for calque to save, and it is not a typical project.

- **Lookups with no file named** (isolated harness, strong models): about half the cost per lookup,
  −54% on Claude Sonnet 5.5 and −51% on Claude Opus 5.5; weaker models read fewer whole files.
- **Full Claude Code, Sonnet 5.5:** lookups by name −26% per successful run; a small edit in a named
  file +9% to +15%.
- **Use:** agents used calque first in 52 of 52 controlled runs, and in 13% to 35% of agents in the
  live work; the cause of the gap is not known.
- **It helps** when the lookup is the job: no file named, large files, weaker models, a list of
  names to find. **It helps little, and can cost a little,** when the lookup is a small part of an
  edit in a file already named. It does nothing for literal text, regular expressions or counts:
  that stays grep's job.

No saving on real work has been shown. Setups, intervals and limits for every figure:
[calque's Measured section](crates/calque/README.md#measured). Results from other projects and
models will be added as they are measured.

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

This runs `calque setup` and `plaz setup` there. They write into the project's `.kulman/` folder,
add the agent configs for the agents they find (the prompt hook only with `--hook`), create
`.agents/memory/` and add `.kulman/` to `.gitignore`. `--scope repo` writes committed files for the
whole team; `--scope local` keeps them to this checkout (what each scope writes, and for which
agent: [calque's setup](crates/calque/README.md#calque-setup)).

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
with `path:start-end` lines, `callers` says who calls a function. Its README holds the CLI, the MCP
server, agent memory, the output format and the measurements.

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
