# nollm-wiki

A deterministic wiki compiler that turns a project (code, notes, docs) into an
agent-navigable knowledge base — generated without an LLM, built for LLM/agent
consumption. Output is byte-identical across machines and thread counts.

Compile a source tree into a cross-linked Markdown wiki with `wiki/`, then
optionally publish it as a browsable static site with `scripts/`.

- **`wiki/`** — the Rust compiler. Start with [`wiki/README.md`](wiki/README.md).
- **`wiki/docs/ARCHITECTURE.md`** — pipeline internals, module map, and determinism rules.
- **`scripts/`** — publish a compiled wiki as a [Quartz](https://quartz.jzhao.xyz/) static site. See [`scripts/README.md`](scripts/README.md).
