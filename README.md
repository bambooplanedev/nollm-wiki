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

## The names

| Name | Say it | The word |
|---|---|---|
| kulman | KOOL-mahn (кульман) | a design bureau's drawing board — the front door: installs the tools, sets up a project |
| calque | /kalk/ | French for tracing paper (Ukrainian «калька») — an exact tracing of your project that agents read |
| plaz | plahz (плаз) | the loft floor where a bureau draws the airframe's real lines full size — the real decisions of each function |
| techlog | TEK-log | an aircraft's technical log — the agent's memory of the project |
| wiki-code | — | plain name: the shared library that walks, parses and indexes code |

## Install

One line fetches `kulman` itself:

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/kulman/releases/latest/download/install.sh | sh

The script downloads the `kulman` archive for your system, checks it against the release's
`SHA256SUMS` (it stops if neither `sha256sum` nor `shasum` is there) and puts the binary at
`~/.kulman-home/bin/kulman`. It writes nothing else and edits no shell profile. Add
`~/.kulman-home/bin` to your `PATH` yourself: agents start `calque` by its name. Releases are
signed: `install.sh` checks the signature of `SHA256SUMS` when `minisign` is installed, and `kulman`
checks every release it reads (see [Checking a release](#checking-a-release)).

Then install the modules:

    kulman install

This puts `calque` and `plaz` (the release's default modules) into `~/.kulman-home/bin`, next to
`kulman`, with the release's `manifest.json` and a `state.json` in `~/.kulman-home`. Nothing is
written outside that folder. Then, in each project:

    cd your-project
    kulman setup --scope repo

This runs `calque setup` and `plaz setup` there. They write into the project's `.kulman/` folder
(the wiki, and the map under `.kulman/map/`), add the agent configs and the prompt hook for the
agents they find, create `.agents/memory/` for agent notes, and add `.kulman/` to `.gitignore`.
`--scope repo` writes committed files for the whole team. `--scope local` writes its files into this
checkout only and lists the ones it created in `.git/info/exclude` instead; the exception is Claude
Code's calque server, which goes through `claude mcp add-json … --scope local` into Claude Code's
own configuration and needs the `claude` CLI. Codex's server goes into the project's own
`.codex/config.toml` with either scope, and Codex loads it only in a project it trusts.

Later, `kulman update` brings everything to the latest release, `kulman setup --scope repo --remove`
undoes a project's setup (with the scope it was set up with) and `kulman remove` deletes what kulman
installed. In a project that the old `wiki` (releases up to 0.6.13) set up, `kulman setup` first
removes the entries it wrote and lists the rest, see
[calque setup](crates/calque/README.md#calque-setup). Commands, flags, exit codes and the layout of
a release: [crates/kulman/README.md](crates/kulman/README.md).

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
Rust and Python are charted; other languages get a "not covered yet" note.

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

Building needs Rust 1.90 or later; the tests also need `jq`.

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
