# Architecture

Internals of the `wiki` compiler, for contributors. For install/usage see
[`../README.md`](../README.md).

## Pipeline overview

`compile()` (`src/lib.rs`) runs seven stages in order, entirely in memory
until each stage's output is complete — nothing touches disk mid-stage. The
`--jobs` thread count changes *when* work finishes, never *what* gets
written: every parallel step collects into an order-preserving `Vec` or a
`BTreeMap`/`BTreeSet` before the next stage (or disk) sees it.

```
walk               →  extract            →  dedup + remap        →  graph + PageRank  →  render               →  manifest             →  lint
src/walk.rs           src/formats/            src/lib.rs             src/graph.rs         src/rewrite.rs          src/manifest.rs         src/lint.rs
                       (Registry)

SourceFile     →      Entity        →         BTreeMap<id,        →  Graph             →  pages (String)     →   Manifest            →  LintReport
(sorted by                                     Entity>                (edges + pagerank)   + .wiki/cache.json      + index.json/.md,
 rel_path)                                                                                  fingerprints            llms.txt, AGENTS.md,
                                                                                                                     graph.json (--emit-json)
```

1. **Walk** (`walk::walk`) — recursively lists `input`, respecting
   `.gitignore`/`.ignore`/hidden-file rules when `respect_ignore` is set, and
   sorts the result by `rel_path`. `compile_inner` then filters out anything
   under `output` (`is_under`, so a nested output dir can't feed its own
   generated pages back in as sources) before extraction runs. Produces
   `Vec<SourceFile>`.
2. **Extract** (`formats::Registry`, in parallel via `rayon`) — dispatches
   each file to the `Extractor` registered for its extension and produces an
   `Entity`. Because the input `Vec` was already sorted, `.collect()`ing the
   `par_iter()` output preserves that order regardless of which thread
   finishes first.
3. **Dedup / remap** (`compile_inner` in `src/lib.rs`) — on an `id` (slug)
   collision, the entity with the lexicographically-first `rel_path` wins;
   any id that collides with a reserved manifest name (`index`, `llms`,
   `agents`, `graph`) is remapped to `<id>_page`, `<id>_page_2`, ... Produces
   `BTreeMap<String, Entity>`.
4. **Graph + PageRank** (`graph::build_graph`) — builds forward/backward
   links by scanning entity bodies for mentions of other entity names (and
   aliases) and by resolving each entity's `imports` to a target id, then
   runs a fixed-iteration PageRank over the link graph. Produces a
   `Graph { edges, pagerank }`.
5. **Render** (`rewrite::render_page`, in parallel) — for each entity,
   reads any existing page to preserve its `## Notes` section, computes a
   content fingerprint (`rewrite::render_fingerprint`), and renders the
   page body. Cross-links are emitted as slug-target, Obsidian-flavored
   wikilinks — `[[slug|Name]]` (e.g. `[[gradient_descent|Gradient
   Descent]]`) — so each link target is a real `<slug>.md` file rather than
   a display name that has to be re-slugified to resolve. Writes only land
   on disk if not in incremental mode, or if the fingerprint changed (see
   [Incremental build](#incremental-build)).
6. **Manifest** (`manifest::build_manifest`) — builds `index.json`,
   `index.md`, `llms.txt`, `AGENTS.md`, and (with `--emit-json`)
   `graph.json` from the final entity map and graph.
7. **Lint** (`lint::lint`) — checks the in-memory rendered `pages` map for
   broken `[[links]]` and orphaned pages; no disk re-read.

## Module map

| Module | Responsibility |
|---|---|
| `src/lib.rs` | `compile()` / `compile_inner()` — orchestrates the pipeline; slug dedup; reserved-name remap; `CompileOptions`/`CompileResult`/`WikiError`. |
| `src/walk.rs` | Filesystem walk with `.gitignore`-aware filtering; produces sorted `Vec<SourceFile>`. |
| `src/formats/mod.rs` | `Extractor` trait, `Registry` (extension → extractor dispatch), `ExtractError`. |
| `src/formats/text.rs` | `TextExtractor` — plain `.txt`, with optional `created:`/`aliases:` front-matter-style lines. |
| `src/formats/markdown.rs` | `MarkdownExtractor` — `.md`/`.markdown`. |
| `src/formats/code.rs` | `CodeExtractor` — tree-sitter-based extraction for Rust/Python/JS/TS/Go; captures only exported/public symbols. |
| `src/formats/summary.rs` | `summarize()` — deterministic, no-LLM one-line summary via a fallback chain (front-matter desc → docstring → first sentence of body → first signature). |
| `src/model.rs` | Core types: `Entity`, `Edges`, `Graph`, `LintReport`, `SourceKind`; `slugify()` — folds a name to an id matching `[a-z0-9_]+` in a single ASCII-fold pass (lowercase, alphanumeric kept, any run of other characters collapsed to one `_`), falling back to an anonymous `page_<hash>` id if nothing alphanumeric survives the fold; `normalize_path()`. |
| `src/graph.rs` | `build_graph()` — mention- and import-based edge detection and PageRank; `orphan_ids()`. |
| `src/hash.rs` | BLAKE3-based `hash_bytes`/`hash_str`/`combine`/`to_hex` used for content hashes and render fingerprints. |
| `src/rewrite.rs` | Page rendering (`render_page`), fingerprinting (`render_fingerprint`), `## Notes` section preservation, atomic file writes (`write_atomic`). |
| `src/cache.rs` | `.wiki/cache.json` — versioned incremental-render cache (`Cache`, `load`, `save`). |
| `src/manifest.rs` | Builds `Manifest` and renders `index.json`, `index.md`, `llms.txt`, `AGENTS.md`, `graph.json`. |
| `src/lint.rs` | `lint()` — in-memory broken-link and orphan-page checks over rendered pages. |
| `src/query.rs` | `Wiki` — loads a compiled output dir for `search()` and `neighbors()` (context-pack) queries; used by the CLI, the MCP server, and as a library API. See [Query internals](#query-internals). |
| `src/serve.rs` | `wiki serve` — MCP server over stdio (`rmcp`): `search`/`neighbors`/`lint` tools and `wiki://page/<id>`, `wiki://index`, `wiki://llms.txt` resources; `WikiState` lazily reloads the compiled wiki when `index.json`'s fingerprint (mtime, len) changes, keeping the last good snapshot if a reload fails. |
| `src/generator.rs` | `generate_corpus()` — deterministic synthetic-corpus generator (SplitMix64 PRNG) used by `wiki generate` and tests. |
| `src/watch.rs` | `watch()`/`recompile_once()` — filesystem-watch-triggered recompilation, ignoring events under `output`. |
| `src/main.rs` | CLI entry point (`clap`): `compile` (with `--watch` to loop via `watch::watch`), `neighbors`, `search`, `lint`, `serve`, `generate`. |

## Determinism rules

Compiling the same input twice — on any machine, with any `--jobs` value —
must produce byte-identical output. These are the rules that make that hold;
breaking any of them reintroduces nondeterminism.

- **Sort inputs at the source.** `walk::walk` sorts files by `rel_path`
  before anything else runs — filesystem directory-listing order is not
  guaranteed stable across platforms/runs, so every later "first one wins"
  decision needs a fixed starting order.
- **Parallel stages always collect into order-preserving containers before
  touching disk.** `rayon`'s `par_iter().collect()` into a `Vec` preserves
  input order regardless of which thread finishes first — so extraction and
  rendering can run in parallel without thread-scheduling order leaking into
  output.
- **Use `BTreeMap`/`BTreeSet`, never `HashMap`/`HashSet`, for anything that
  is iterated into output.** Rust's default hasher is randomized per
  process (`RandomState`), so `HashMap` iteration order — and therefore any
  order derived from it — would differ from run to run even with identical
  input.
- **Slug-collision dedup keeps the entity with the lexicographically-first
  `rel_path`.** Combined with the sorted-walk guarantee above, this makes
  the winner a pure function of the input paths, not of extraction
  completion order.
- **Reserved-name remap iterates the entity `BTreeMap` in sorted key
  order.** The remap (`index` → `index_page`, etc.) walks entries in that
  fixed order, so the same input always produces the same remapped ids
  regardless of thread count.
- **Content hashing uses BLAKE3** (`hash::hash_bytes`, `hash::combine`),
  not a randomized or platform-dependent hasher — BLAKE3 has no per-process
  seed, so identical bytes always hash identically, on any machine.
  `combine()` additionally length-prefixes each part before hashing, so
  `["ab","c"]` and `["a","bc"]` never collide.
- **PageRank runs a fixed 40 iterations over a `BTreeMap<String, Edges>`**
  (`graph::pagerank`) — no convergence-threshold early exit and no
  hash-map-order iteration, so rank computation is a pure function of the
  graph, not of float-convergence timing or map iteration order.
- **Float comparisons use `f64::total_cmp` with an id tie-break, never
  `partial_cmp`.** `partial_cmp` returns `None` for incomparable values and
  gives ties no defined order; `total_cmp` gives every `f64` a total order,
  and the id tie-break (in `query.rs` sort keys for search-hit ranking) means
  equal scores still resolve to one fixed order instead of falling back to
  undefined behavior.
- **The synthetic corpus generator uses a hand-rolled, explicitly-seeded
  SplitMix64 PRNG** (`generator::generate_corpus`), not `rand`'s
  OS/thread-seeded RNG — the same `--seed` must produce the same corpus on
  any platform or Rust version, which an OS-entropy-seeded generator cannot
  guarantee.
- **Path containment (`is_under`, in `src/lib.rs`) compares lexically
  absolute paths** (`std::path::absolute`, no filesystem access or symlink
  resolution) rather than canonicalizing — canonicalization depends on the
  filesystem's current state (symlinks, mounts), which varies by machine;
  the lexical form doesn't.

## Incremental build

With `--incremental`, `compile()` loads `.wiki/cache.json` (via
`cache::load`) instead of starting from an empty cache. The cache is
guarded by three fields that must all match the current run before it is
trusted at all — `version` (`cache::CACHE_VERSION`), `hash_algo`
(currently `"blake3"`), and `tool_version` (`CARGO_PKG_VERSION`). If any of
them mismatch, `load()` silently falls back to `Cache::fresh()`, forcing a
full re-render rather than trusting a cache written by an incompatible
compiler version or hash scheme.

For each entity, the render stage computes a fingerprint
(`rewrite::render_fingerprint`) that mixes the entity's content, its edges,
the full entity map (so a neighbor's change can affect a page that links to
it), and any preserved `## Notes` text, via the order-sensitive
`hash::combine`. A page is only rewritten to disk
(`rewrite::write_atomic` — temp file + rename, so an interrupted run can't
leave a half-written page) if `cache.needs_render(id, fingerprint)` is true,
i.e. the stored fingerprint for that id differs (or there is none yet).

After rendering, any cache entry whose id is no longer present in the
current entity set is pruned: the corresponding page file is deleted from
`output`, and `cache.retain_ids()` drops it from the cache before it is
saved back to `.wiki/cache.json`. This keeps stale pages from a deleted or
renamed source from lingering in the output directory across incremental
runs.

**Notes preservation:** before rendering a page, `rewrite::read_preserved_notes`
reads the *existing* file at that output path (if any) and extracts its
`## Notes` section via `rewrite::parse_sections`. That text is passed back
into `render_page` and re-emitted verbatim in the new version of the page —
so hand-written notes under a page's `## Notes` heading survive
regeneration, incremental or not, as long as the heading itself isn't
renamed.

## Query internals

`Wiki::load` (`src/query.rs`) reads only `index.json` — metadata plus
adjacency; page bodies are read on demand from `<id>.md`.

**Search** (`Wiki::search`) is case-insensitive, tokenized, with AND
semantics:

- The query is lowercased, split on whitespace, each piece's edge
  punctuation trimmed (interior characters like `_` and `:` survive),
  empties dropped, duplicates removed. Empty or punctuation-only queries
  return no hits.
- Every token must match at least one field — name, alias, summary, or
  body; a page missing any token is excluded entirely.
- Score = per-token field weights (name 3.0, alias 2.0, summary 1.5,
  body 1.0) + a graded occurrence bonus (0.1 per body occurrence across
  all tokens, capped at 20 occurrences) + the page's PageRank as a
  tiebreak. Hits sort descending by score (`total_cmp`), then ascending
  by id.
- The searched body text is the rendered page's parsed sections minus
  generated chrome (`Metadata`, `Related`, `Referenced By`, `Notes`) —
  subtractive on purpose, so text under a doc's own embedded `## `
  subheadings stays searchable. Occurrence counting and snippet
  extraction share one lowercased scan of that content.
- Hits whose body matched carry a `snippet`: a deterministic excerpt
  around the earliest token occurrence (60 chars of context per side,
  whitespace runs collapsed, `…` on truncated edges). Title/alias/
  summary-only hits have `snippet: None` — the summary explains those.

**Neighbors** (`Wiki::neighbors`) BFS-collects ids to `depth` hops over
both edge directions, then builds a budgeted context pack:

- `--max-tokens` is a hard ceiling on the pack's estimated size, using
  the same chars/4 rule as `manifest::token_estimate`; every block is
  charged at the size of the text actually emitted, so the concatenated
  pack can't overshoot the ceiling.
- The target page comes first — in full when it fits, otherwise degraded
  to a title + summary block pointing at `wiki://page/<id>`. The degraded
  block is the one floor exception: it is always emitted, even when a
  pathologically small budget cannot contain it.
- Neighbor admission walks candidates in descending centrality
  (skip-not-break): a neighbor that doesn't fit the remaining budget is
  skipped and the walk continues, so the kept set favors high-centrality
  neighbors over maximum cardinality. `--max-nodes` truncates the
  candidate list before admission.
- Emission order is ascending PageRank — the highest-centrality neighbor
  lands last, closest to the end of the pack ("lost in the middle"
  placement). `--full` swaps neighbor summary blocks for full page
  bodies.

## Extending: add a format extractor

1. Implement the `Extractor` trait (`src/formats/mod.rs`) in a new
   `src/formats/<name>.rs`:
   ```rust
   pub trait Extractor: Send + Sync {
       fn extensions(&self) -> &[&str];
       fn extract(&self, rel_path: &str, text: &str) -> Entity;
   }
   ```
   `extract` only needs to fill in the semantic fields of `Entity`
   (`name`, `aliases`, `body`, `kind`, `summary`, `symbols`, `imports`,
   etc.) — `source_path` and `content_hash` are filled in by the
   `Registry` after dispatch, not by the extractor itself.
2. Register it in `Registry::with_defaults()` in `src/formats/mod.rs`,
   alongside `TextExtractor`, `MarkdownExtractor`, and `CodeExtractor`:
   ```rust
   reg.register(Arc::new(YourExtractor));
   ```
   `Registry::register` maps every extension from `extensions()` to the
   extractor, so one extractor can claim multiple extensions.
3. If the extractor needs a heavy dependency (a PDF parser, an OCR engine,
   an audio-transcription library), feature-gate it in `Cargo.toml` rather
   than adding it to the default dependency set. The `pdf`, `ocr`, and
   `audio` features already exist for exactly this purpose:
   ```toml
   [features]
   default = []
   pdf = []
   ocr = []
   audio = []
   full = ["pdf", "ocr", "audio"]
   ```
   These are currently **empty seams** — no extractor is registered under
   them yet, and no backend crate is pulled in. Adding one means adding the
   real dependency under that feature name, registering the extractor
   behind `#[cfg(feature = "pdf")]` (etc.) at the `// Feature seams` comment
   in `Registry::with_defaults()`, and building with
   `cargo build --features pdf` (or `--features full`) to opt in.

**Export-only contract for code extraction:** `CodeExtractor`
(`src/formats/code.rs`) only captures symbols that are part of a language's
public/exported surface — `pub` items in Rust/Go, symbols wrapped in an
`export_statement` in JS/TS. Private helpers and unexported items are
deliberately excluded from `Entity::symbols`; a new language extractor
should preserve this contract rather than dumping every definition in a
file.

**Rust test-module stripping:** before body assembly,
`strip_rust_test_modules` (`src/formats/code.rs`) removes each
`#[cfg(test)]`-annotated `mod` item — including the contiguous attribute
run above it — from the Rust source shown in `## Body`, replacing it with
a one-line marker: `// [tests omitted: mod <name>, <N> lines]`. The strip
applies to the body text only; `symbols` and `imports` are still captured
from the raw source (a known limitation, tracked as dogfood finding 13).

## Testing

- **Unit tests** live in `#[cfg(test)] mod tests` blocks at the bottom of
  the source file they test (e.g. `src/cache.rs`, `src/hash.rs`,
  `src/lib.rs`, `src/formats/code.rs`) — colocated with the code, not in a
  separate tree.
- **Integration tests** live in `tests/`:
  - `tests/end_to_end.rs` — full `compile()` runs against small in-memory
    corpora: artifact presence, cross-linking, reserved-name remapping,
    and `output_is_deterministic_across_jobs`, which compiles the same
    20-entity corpus with `jobs: Some(1)` and `jobs: Some(8)` and asserts
    the resulting `index.json` bytes are identical — the concrete
    regression test for the determinism rules above.
  - `tests/query.rs` — `Wiki::load`, `search`, and `neighbors` against a
    compiled output directory.
  - `tests/cli.rs` — the `wiki` binary's subcommands via `assert_cmd`.
  - `tests/search_quality.rs` — regression tests for the 2026-07-14
    search-quality cycle: the dogfood checklist queries that failed
    pre-fix, AND semantics, and occurrence-graded ranking, over in-test
    fixtures.
  - `tests/query_list_pages.rs` — `Wiki::list_pages` id/title ordering
    (backs the MCP server's `resources/list`).
  - `tests/mcp_serve.rs` — end-to-end MCP: spawns `wiki serve` and speaks
    raw newline-delimited JSON-RPC over stdio, covering tools and
    resources.
- **Snapshot test:** `tests/snapshot.rs` uses `insta` to pin the exact
  rendered Markdown of one page from a seeded generated corpus
  (`generate_corpus(&input, 12, 42)`), with `source_hash` lines redacted so
  hash-format churn doesn't force a snapshot update for unrelated reasons.
  Snapshots live under `tests/snapshots/`; review changes with
  `cargo insta review` before accepting them.
- **Gate trio** — run all three before sending a change:
  ```bash
  cargo test
  cargo clippy --all-targets -- -D warnings
  cargo fmt --check
  ```
