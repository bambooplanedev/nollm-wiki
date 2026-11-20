# wiki-code

**wiki-code**: a plain name — the shared library that walks, parses and indexes code.

A library crate of the [nollm-wiki project](../../README.md), shared by [`wiki`](../wiki/README.md) and
[plaz](../plaz/README.md): the item model, the source tree walk, tree-sitter extraction for each
language, the index that call sites resolve against, and import resolution. The contract a consumer
may rely on is written at the top of [`src/lib.rs`](src/lib.rs); the grammar versions are pinned
exactly in `Cargo.toml`, since a grammar bump changes what is extracted.

It has no binary and is not published on crates.io. `cargo test -p wiki-code` runs its tests.
