# wiki

The Rust crate behind nollm-wiki: the `wiki` binary and the library it wraps. What it does, how to
install it, the CLI and the MCP server are in the [repository README](../README.md).

## Build

    cargo build --release      # target/release/wiki
    cargo test

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
`list_pages()` (`(id, title)` pairs, ascending by id) — the calls the MCP server's resources use.

## Determinism

Compiling the same input twice — on any machine, with any `--jobs` value — produces byte-identical
output, across the ordering of parallel work, hashing, float comparisons and path handling. The
rules that make this hold are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — pipeline internals, module map, determinism rules,
  and how to extend the compiler.
- [docs/KNOWN_GAPS.md](docs/KNOWN_GAPS.md) — where results fall short and what agents do with the
  wiki.
