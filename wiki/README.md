# wiki

Deterministic, no-LLM wiki compiler. Turns a folder of source files (`.txt`, `.md`,
code) into a cross-linked Markdown wiki plus machine-readable artifacts
(`index.json`, `llms.txt`, `AGENTS.md`) for cheap LLM/agent consumption.

## Build

```bash
cargo build --release
```

## Use

```bash
wiki generate demo_raw --files 50        # optional: synthetic corpus
wiki compile demo_raw demo_wiki --incremental
wiki search "attention" --dir demo_wiki
wiki neighbors gradient_descent --depth 1 --dir demo_wiki
```

Optional format backends are feature-gated and off by default:
`cargo build --features full` (pdf/ocr/audio seams — not yet implemented).
