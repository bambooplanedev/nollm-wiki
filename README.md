# nollm-wiki

A deterministic wiki compiler that turns a project (code, notes, docs) into an
agent-navigable knowledge base — generated without an LLM, built for LLM/agent
consumption. Output is byte-identical across machines and thread counts.

Compile a source tree into a cross-linked Markdown wiki with `wiki/`, then
optionally publish it as a browsable static site with `scripts/`.

- **`wiki/`** — the Rust compiler. Start with [`wiki/README.md`](wiki/README.md).
- **`wiki/docs/ARCHITECTURE.md`** — pipeline internals, module map, determinism rules, and query internals (search/neighbors).
- **`scripts/`** — publish a compiled wiki as a [Quartz](https://quartz.jzhao.xyz/) static site. See [`scripts/README.md`](scripts/README.md).

## Self-hosting

This repo compiles itself. `scripts/selfhost.sh` always runs
`cargo build --release` (a no-op when sources are unchanged, so the binary
can never go stale), compiles the repository into `.wiki/` (gitignored;
incremental on re-runs), and then runs `wiki lint` on the result — a broken
wikilink fails the script:

```bash
scripts/selfhost.sh
```

The checked-in `.mcp.json` registers `wiki serve --source .` as a
project-scoped MCP server named `nollm-wiki`, so agent sessions started in
this directory can search, expand, and lint the self-hosted wiki. The server
compiles into `.wiki/` itself and recompiles on edits, so no refresh step is
needed. In a fresh clone, run `scripts/selfhost.sh` once so the binary exists
before starting an agent session.
