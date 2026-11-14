**English** · [Українська](README.uk.md)

# kulman

**kulman**, said KOOL-mahn (кульман): a design bureau's drawing board — the front door of this set:
it installs the tools and sets up a project.

[![CI](https://github.com/bambooplanedev/kulman/actions/workflows/rust.yml/badge.svg?branch=main)](https://github.com/bambooplanedev/kulman/actions/workflows/rust.yml)
[![Release](https://img.shields.io/github/v/release/bambooplanedev/kulman)](https://github.com/bambooplanedev/kulman/releases)
[![Rust 1.90+](https://img.shields.io/badge/rust-1.90%2B-orange?logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

kulman is a set of tools for coding agents and the people who work with them, named after the
drawing boards and floors of an aircraft design bureau. `kalka` compiles a project into a map an
agent can search — where each function lives, what calls it, which lines to read — and `plaz` draws
the real decisions of each function for people to read. Neither calls a model: the same sources give
byte-identical output, and `kulman` installs both and sets them up in a project with one command
each.

## The names

| Name | Say it | The word |
|---|---|---|
| kulman | KOOL-mahn (кульман) | a design bureau's drawing board — the front door: installs the tools, sets up a project |
| kalka | KAHL-kah (калька) | tracing paper — an exact tracing of your project that agents read |
| plaz | plahz (плаз) | the loft floor where a bureau draws the airframe's real lines full size — the real decisions of each function |
| techlog | TEK-log | an aircraft's technical log — the agent's memory of the project |
| wiki-code | — | plain name: the shared library that walks, parses and indexes code |

## Install

The current release is 0.7.0. One line fetches `kulman` itself:

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/kulman/releases/latest/download/install.sh | sh

The script downloads the `kulman` archive for your system, checks it against the release's
`SHA256SUMS` (it stops if neither `sha256sum` nor `shasum` is there) and puts the binary at
`~/.kulman-home/bin/kulman`. It writes nothing else and edits no shell profile. Add
`~/.kulman-home/bin` to your `PATH` yourself: agents start `kalka` by its name.

Then install the modules:

    kulman install

This puts `kalka` and `plaz` (the release's default modules) into `~/.kulman-home/bin`, next to
`kulman`, with the release's `manifest.json` and a `state.json` in `~/.kulman-home`. Nothing is
written outside that folder. Then, in each project:

    cd your-project
    kulman setup --scope repo

This runs `kalka setup` and `plaz setup` there. They write into the project's `.kulman/` folder (the
wiki, and the map under `.kulman/map/`), add the agent configs and the prompt hook for the agents
they find, create `.agents/memory/` for agent notes, and add `.kulman/` to `.gitignore`. `--scope repo`
writes committed files for the whole team; `--scope local` keeps everything to this checkout and
lists what it created in `.git/info/exclude` instead.

Later, `kulman update` brings everything to the latest release, `kulman setup --remove` undoes a
project's setup and `kulman remove` deletes what kulman installed. Commands, flags, exit codes and
the layout of a release: [crates/kulman/README.md](crates/kulman/README.md).

### Checking a release

A release is checked by SHA-256 only: `install.sh` and `kulman` compare each archive with the sums
in the release's `SHA256SUMS` and `manifest.json`. Releases are not signed: the sums show that a
download is the file the release lists, not who made the release. To check by hand, with the archives and
`SHA256SUMS` in one folder, run `sha256sum -c --ignore-missing SHA256SUMS` (on macOS
`shasum -a 256 -c --ignore-missing SHA256SUMS`).

## Modules

**[kulman](crates/kulman/README.md)** installs, updates and removes the modules and runs their
setup in a project. It is the installer, not a module.

**[kalka](crates/kalka/README.md)** compiles a project — code, notes, docs — into a cross-linked
Markdown wiki and a symbol map with line ranges, and serves it to agents over MCP: `search` answers
with `path:start-end` lines, `callers` says who calls a function. `kalka setup` wires it into Claude
Code, pi, Cursor and Codex. Its README holds the CLI, the MCP server, agent memory, the output
format and the measurements.

**[plaz](crates/plaz/README.md)** writes, for every function, a Markdown page with a decision
flowchart: the literal condition of each branch, who calls the function and what each call reaches.
Rust and Python are charted; other languages get a "not covered yet" note.

**[techlog](crates/techlog/README.md)** is a library: where agent memory lives — the project's
`.agents/memory/` and the folders agents keep outside it — and who wrote each note. kalka uses it.

**[wiki-code](crates/wiki-code/README.md)** is the library kalka and plaz share: it walks a source
tree, extracts functions and types with tree-sitter, and indexes calls and imports.

## Requirements

- A release runs on macOS on Apple silicon (`aarch64-apple-darwin`) and on Linux x86_64
  (`x86_64-unknown-linux-musl`). The Linux build is static and runs on glibc and musl systems
  alike. Those are the only two builds; anything else, Intel Macs included, builds from source.
  Windows is not tested.
- `install.sh` needs `curl`, `tar`, `mktemp` and `sha256sum` or `shasum`.
- For agent use: an MCP client — Claude Code, pi, Cursor, Codex, Claude Desktop or another. See
  [kalka's requirements](crates/kalka/README.md#requirements).

## Building from source

Building needs Rust 1.90 or later; the tests also need `jq`.

    git clone https://github.com/bambooplanedev/kulman
    cd kulman
    cargo install --path crates/kalka      # into ~/.cargo/bin
    cargo install --path crates/plaz
    cargo install --path crates/kulman

    cargo test
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

`cargo install` puts the binaries in `~/.cargo/bin`; a `kulman` built this way still installs the
modules of a published release. Pushing a `v*` tag that equals the workspace version
builds the macOS and Linux archives, checks that they install, and makes a draft GitHub Release with
their `SHA256SUMS`.

## License

[MIT](LICENSE)
