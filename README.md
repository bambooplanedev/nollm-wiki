# nollm-wiki

A deterministic wiki compiler that turns a project (code, notes, docs) into an
agent-navigable knowledge base — generated without an LLM, built for LLM/agent
consumption. Output is byte-identical across machines and thread counts.

- **`wiki/`** — the Rust compiler. Start with [`wiki/README.md`](wiki/README.md).
- **`wiki/docs/ARCHITECTURE.md`** — pipeline internals, module map, and determinism rules.
- **`docs/superpowers/`** — design specs and implementation plans.
