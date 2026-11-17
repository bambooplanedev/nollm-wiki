**English** · [Українська](README.uk.md)

# plaz

**plaz**, said plahz (плаз): the loft floor where a bureau draws the airframe's real lines full
size — the real decisions of each function.

`plaz` draws the real decisions of each function: for every function a Markdown page with a decision flowchart (Mermaid) that holds the literal condition of each `if`, `match` arm and loop, the comments attached to it, who calls the function, and what each call reaches. It calls no model: the same sources give byte-identical pages.

The map is for the person who owns the code, to read the business logic and set tasks from it. `plaz setup` tells agents nothing about it: it adds no MCP server, skill, instruction line or hook, and the map's folder is ignored by git.

It is a module of the [kulman set](../../README.md), beside [calque](../calque/README.md). It depends on `wiki-code` of this workspace by path and nothing of calque's wiki. Its version is the workspace's.

**Preview.** Rust, Python, C and C++ are charted; other languages get a "not covered yet" note. `kulman install` installs it with the release (see [Install](../../README.md#install)); to build it yourself, see below.

## Build

    cargo build --release      # ../../target/release/plaz, at the workspace root
    cargo test

## Commands

    plaz build [--source DIR] [--vault DIR]
    plaz setup [--source DIR] --scope repo|local [--remove]

`plaz build` reads the code under `--source` (default `.`) and writes the pages to `<source>/.kulman/map/`, or to `map/` under `--vault`. Pages carry the front matter `tags: ["plaz"]` and `kind: map`; a file at a page path without it is left alone and counted. Unchanged pages are not rewritten, pages of files that are gone are removed, and the output is the same on every run. A tree with no code file leaves the folder as it is, with a warning. Directories named `third_party`, `third-party`, `vendor`, `vendored` or `node_modules` are skipped; a line in a `.ignore` file (such as `extern/`) skips any other bundled code. Code references become wikilinks to calque's code pages when the vault holds a compiled calque wiki, and plain `path:start-end` otherwise.

`plaz setup` builds the map and makes sure `.kulman/` is ignored by git:

- `--scope repo` adds the line `.kulman/` to `<source>/.gitignore`, shared with the team.
- `--scope local` adds it to the repository's `info/exclude`, for this clone only. Outside a git repository it builds the map, says `not a git repository: nothing to exclude` and changes no file.
- A line in either file counts: nothing is added twice.
- `--remove` deletes only the map's own pages and the folders left empty under `.kulman/map/`, `map/` itself included once it is empty; the ignore line stays, and calque's files in `.kulman/` are not touched.

`plaz setup` and calque's `calque setup` can both run in one project: each keeps to its own pages.

## Design

[docs/design.md](docs/design.md) says what the map shows and what it does not claim; [docs/open-questions.md](docs/open-questions.md) holds the known limitations, the measurements and what is planned (more languages, state diagrams, "what changed in the logic", publishing).
