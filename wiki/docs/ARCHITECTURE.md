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
walk               →  extract            →  qualify + remap      →  graph + PageRank  →  render               →  manifest             →  lint
src/walk.rs           src/formats/            src/lib.rs             src/graph.rs         src/rewrite.rs          src/manifest.rs         src/lint.rs
                       (Registry)

SourceFile     →      Entity        →         BTreeMap<id,        →  Graph             →  pages (String)     →   Manifest            →  LintReport
(sorted by                                     Entity>                (edges + pagerank)   + .wiki/cache.json      + index.json/.md,
 rel_path)                                                                                  fingerprints            llms.txt, AGENTS.md,
                                                                                                                     graph.json (--emit-json)
```

1. **Walk** (`walk::walk`) — recursively lists `input`, respecting
   `.gitignore`/`.ignore`/hidden-file rules when `respect_ignore` is set, and
   sorts the result by `rel_path`. A `keep` predicate (the registry's
   extension set) is consulted on the path *before* a file is opened, so an
   unhandled file is never read into memory. `compile_inner` then filters out anything
   under `output` (`is_under`, so a nested output dir can't feed its own
   generated pages back in as sources) before extraction runs. Produces
   `Vec<SourceFile>`.
2. **Extract** (`formats::Registry`, in parallel via `rayon`) — dispatches
   each file to the `Extractor` registered for its extension and produces an
   `Entity`. Because the input `Vec` was already sorted, `.collect()`ing the
   `par_iter()` output preserves that order regardless of which thread
   finishes first. Tree-sitter queries are compiled once per process
   (`QUERIES`, a `LazyLock`), and `validate_queries()` forces them at the top
   of `compile_inner`, so a bad query fails before any page is written rather
   than inside a worker with other pages already on disk.
3. **Qualify / remap** (`compile_inner` in `src/lib.rs`) — an extractor
   names a page from its own file alone and cannot see the rest of the tree,
   so ids are made unique here, where every entity is in hand.
   `disambiguate_ids` prefixes parent directory segments to the names that
   would otherwise share a slug, one segment per round until the clash
   clears (`app/api/models.py` → `Api Models` → `api_models`); a name that
   is already unique is never touched. Whatever still collides afterwards —
   a file at the project root with no segment left to take, or two paths
   equal but for case — falls through to the older rule, where the entity
   with the lexicographically-first `rel_path` wins. Any id that collides
   with a reserved manifest name (`index`, `llms`, `agents`, `graph`) is then
   remapped to `<id>_page`, `<id>_page_2`, ..., with a warning on stderr. Produces
   `BTreeMap<String, Entity>`.
4. **Graph + PageRank** (`graph::build_graph`) — builds forward/backward
   links three ways. Mention edges: entity bodies are scanned for other
   entities' names and aliases (the phrase index); a candidate whose target
   is a **code page** is kept only when the body refers to that module in
   code shape (`refers_to_module`: a `stem::`/`::stem` path, a `mod stem;`
   declaration, or the filename `stem.rs`), never by prose or a bare
   backticked word — a one-word module name such as `Text` would otherwise
   match every English use of the word. Import edges: every segment of an
   import string that equals a code page's module stem
   (`formats::code::module_stem`, the same rule that names the page) or a
   name in its `defined` list (`ImportResolver`); a `::` path is followed
   only under `crate`, `super`, `self`, or a local crate root, the
   directory holding a `src/lib.rs`/`src/main.rs`, so `rmcp::model` never
   reaches `model`. Link edges: each inline markdown link `[text](target)`
   resolved lexically against the linking file's directory to another
   entity's `source_path` (`resolve_link`; external schemes, bare anchors,
   and targets above the root are ignored). Then it runs a fixed-iteration
   PageRank over the link graph. Produces a `Graph { edges, pagerank }`.
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
   broken `[[links]]` and orphaned pages; no disk re-read. Links are scanned
   on `rewrite::mask_code` output, so a `[[link]]` inside a fenced block or
   inline code span is never reported, and a code page's `## Body` (verbatim
   source, per its `## Metadata` kind) is dropped from the scan altogether.
   The CLI exits 1 on any broken link; orphans are advisory.

## Module map

| Module | Responsibility |
|---|---|
| `src/lib.rs` | `compile()` / `compile_inner()` — orchestrates the pipeline; `disambiguate_ids` name qualification; slug dedup; reserved-name remap; `CompileOptions`/`CompileResult`/`WikiError`. |
| `src/walk.rs` | Filesystem walk with `.gitignore`-aware filtering; produces sorted `Vec<SourceFile>`. |
| `src/formats/mod.rs` | `Extractor` trait, `Registry` (extension → extractor dispatch), `ExtractError`. |
| `src/formats/text.rs` | `TextExtractor` — plain `.txt`, with optional `created:`/`aliases:` front-matter-style lines; a `# Heading` or an ALL-CAPS first line becomes the title. |
| `src/formats/markdown.rs` | `MarkdownExtractor` — `.md`/`.markdown`. |
| `src/formats/code.rs` | `CodeExtractor`, shared extraction core: `LangSpec`, `QUERIES`, `extract_code`, `build_signature`, `render_span`, `default_cut`, `tidy_punctuation`, `collapse_runs`, `Placement`, `Shape`, `Rank`, `ItemKind`. Holds no language's grammar node kinds — those live behind the `Shape` hook in each language's module. |
| `src/formats/extract_rust.rs` | The Rust `LangSpec` and `rust_shape`: bare-`pub` visibility gating, owner qualification through `impl`/`trait` scopes and through struct/union/enum bodies, `#[macro_export]` gating for `macro_rules!`, `#[cfg(test)]` module stripping, and the `enum_variant`/`macro_definition`/`const`/`static`/`type`/`mod` signature shapes. |
| `src/formats/extract_python.rs` | The Python `LangSpec` and `python_shape`: class-chain owner qualification, `__all__` handling, module docstring extraction, and the `assignment` signature shape. |
| `src/formats/extract_simple.rs` | JS, TS, and Go — three specs with no owner resolution, gated by `export_statement` or (Go) a leading-capital naming convention. |
| `src/formats/summary.rs` | `summarize()` — deterministic, no-LLM one-line summary via a fallback chain (front-matter desc → docstring → first sentence of body → first signature). The sentence is cut from the whole opening paragraph (consecutive non-empty lines joined by a space), so a sentence wrapped at 80 columns in a `//!` block or README prose is not truncated at the line break. A sentence ends only at `.`/`!`/`?` followed by whitespace or end of line (so `index.json` does not end one), and lines opening "This document/page/module/file/note" are skipped as boilerplate. |
| `src/model.rs` | Core types: `Entity`, `Edges`, `Graph`, `LintReport`, `SourceKind`; `title_case()` — the one casing rule every name-deriving path shares; `slugify()` — folds a name to an id matching `[a-z0-9_]+` in a single ASCII-fold pass (lowercase, alphanumeric kept, any run of other characters collapsed to one `_`), falling back to an anonymous `page_<hash>` id if nothing alphanumeric survives the fold; `normalize_path()`. |
| `src/graph.rs` | `build_graph()` — mention edges (phrase index, filtered by `refers_to_module` for code targets), import edges (`ImportResolver`: stem and `defined`-name segments under local roots), markdown-link edges (`resolve_link`), and PageRank; `orphan_ids()`. |
| `src/hash.rs` | BLAKE3-based `hash_bytes`/`hash_str`/`combine`/`to_hex` used for content hashes and render fingerprints. |
| `src/rewrite.rs` | Page rendering (`render_page`), fingerprinting (`render_fingerprint`), `## Notes` section preservation, atomic file writes (`write_atomic`); code-block masking, byte-offset preserving: `mask_fenced_code` backs `parse_sections` (so a `##` heading quoted in a fence never becomes a section, for rendering and search alike), and `mask_code` adds `mask_inline_code` on top for lint's link scan. |
| `src/cache.rs` | `.wiki/cache.json` — versioned incremental-render cache (`Cache`, `load`, `save`). |
| `src/manifest.rs` | Builds `Manifest` and renders `index.json`, `index.md`, `llms.txt` (pages at or above the median PageRank under `## Docs`, the rest under `## Optional`), `AGENTS.md`, `graph.json`. |
| `src/lint.rs` | `lint()` — in-memory broken-link and orphan-page checks over rendered pages. |
| `src/query.rs` | `Wiki` — loads a compiled output dir for `search()` and `neighbors()` (context-pack) queries, plus `page()`/`has_page()`/`list_pages()` for the MCP resources; used by the CLI, the MCP server, and as a library API. See [Query internals](#query-internals). |
| `src/serve.rs` | `wiki serve` — MCP server over stdio (`rmcp`): `search`/`neighbors`/`lint` tools and `wiki://page/<id>`, `wiki://index`, `wiki://llms.txt` resources; `WikiState` lazily reloads the compiled wiki when `index.json`'s fingerprint (mtime, len) changes, keeping the last good snapshot if a reload fails. |
| `src/generator.rs` | `generate_corpus()` — deterministic synthetic-corpus generator (SplitMix64 PRNG) used by `wiki generate` and tests. |
| `src/watch.rs` | `watch()`/`recompile_once()` — filesystem-watch-triggered recompilation, ignoring events under `output`, debounced 150 ms; prints `watching … (Ctrl-C to stop)` and `recompiled: N pages (M written)` on stderr. |
| `src/main.rs` | CLI entry point (`clap`): `compile` (with `--watch` to loop via `watch::watch`), `neighbors`, `search`, `lint`, `serve`, `generate`. |

**The `extract_*.rs` names are two words on purpose, not tidiness.** Page
titles are derived from file stems and are inputs to both the graph and
search. When these names were chosen, a mention edge needed only the
title's words in prose, so a page titled `Rust` (from a hypothetical
`rust.rs`) drew an edge from every prose mention of "rust" — measured at 9
files for `Rust`, 2 for `Python`, 1 for `Simple`, on this repo alone — and
`code_rust.rs` failed too, because the compiler's own `SourceKind` string
`code:rust` tokenizes to the adjacent words `code` and `rust`. That graph
cost is gone: a mention links a code page only in code shape (stage 4
above). The names stay two words for search, where a title matches a query
by word prefix, so a page titled `Rust` or `Python` would rank for every
query that names the language rather than the extractor.

**A directory-module page takes its directory's name.** `mod.rs` and
`__init__.py` say nothing about the module they open — every importer refers
to them by the directory (`mod common;`, `from pkg import x`). Naming such a
page from its own stem produced `Mod` / `Init`, and where several existed
they collided and were qualified to `common_mod` / `formats_mod`: names
appearing in no other page's body, so the page was unreachable by the
phrase index and by import resolution alike. `tests/common/mod.rs` was a
permanent orphan for exactly this reason. Naming it `Common` lets `mod
common;` and `common::write` link it. The one-word prose cost this used to
carry ("A common mistake is tuning …" drew an edge) is gone since mention
edges into code pages must be code-shaped; the `extract_*.rs` naming above
still matters for search, where a one-word title matches by prefix.

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
- **Name qualification depends on the set of files, not their order.**
  `disambiguate_ids` reads only each entity's `(name, source_path)` pair and
  extends every member of a colliding group together, so the ids it produces
  are the same whichever order extraction finished in. It terminates because
  a path has finitely many segments and a round that extends nobody stops the
  loop.
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
- **PageRank runs a fixed 40 iterations at damping 0.85 over a `BTreeMap<String, Edges>`**
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
- **Grammar shape is part of the output contract.** Extraction depends on
  tree-sitter field names (`trait:`, `type:`, `value:`, `body:`) and node kinds
  (`declaration_list`, `const_item`, `static_item`, `type_item`,
  `function_signature_item`). `Cargo.toml` carets
  `tree-sitter-rust = "0.24"`, so `Cargo.lock` is what guarantees byte-identical
  output across machines; a grammar bump is an output-affecting change and must
  be reviewed as one.
- **Rust and Python guard module level with opposite conventions, on
  purpose.** `rust_placement` (`extract_rust.rs`) is an allow-list of item
  containers (`source_file`, `mod_item`, `declaration_list`, `impl_item`,
  `trait_item`, plus the type bodies that hold fields and variants:
  `struct_item`, `union_item`, `enum_item`, `field_declaration_list`,
  `enum_variant_list`); `python_placement` (`extract_python.rs`) is a deny-list of
  one node kind (`function_definition`/`lambda`). The grammars are mirror
  images — Rust has many kinds of item container, Python's module level has
  exactly one excluder — so the two rules fail in opposite directions: a new
  tree-sitter node kind that can host a definition is silently *admitted* by
  Python's rule and silently *rejected* by Rust's. A `tree-sitter-rust` or
  `tree-sitter-python` grammar bump must be reviewed under both conventions,
  not just re-checked against the existing test fixtures.
- **`## Exports` is ordered by a grouping key, `(group, kind, name)`, not by
  plain lexicographic sort of the rendered signature — for every language.**
  `group` is the owner for a member and the item's own name otherwise, so
  `class Article` and `Article.title: str` share the group `Article` instead
  of scattering under `@` < `A` < `d`. `kind` ranks `FreeDef`/`FreeValue`/
  `Header` ahead of `Member`, so a class or an `impl` header leads its own
  members. Only the *order* changes: `Entity::symbols` stays a plain sorted
  `Vec<String>`, and `dedup_by` (keyed on the signature alone) still merges
  equal signatures, since equal signatures have equal group/kind/name too.
  The summary fallback deliberately does **not** read this order — it picks
  the smallest signature within each `ItemKind`, independent of how
  `## Exports` displays — because grouping sorts on the bare name, and an
  uppercase type name outranks a lowercase function name there even when the
  function is what the module is about.
- **The `ItemKind::FreeValue` rank (the classification this bullet's `kind`
  depends on) is not itself ordering-only.** It can change which signature a
  Rust page picks as its *summary*, not just where `## Exports` displays it: a
  Rust module whose only free items are `const`/`static`/`type` and which has
  no `//!` doc comment will summarize differently than it would under plain
  lexicographic order, because an uppercase constant would otherwise outrank a
  lowercase `impl` header. Inert on this repository — pinned by
  `a_const_only_module_without_docs_prefers_an_impl_header_to_the_const`
  (`extract_rust.rs`) — but real for any Rust crate with that shape, and it
  landed in the same commit as the Python symbols overhaul, not the later
  grouping-order commit.
- **Import resolution tie-breaks on the lexicographically smallest
  `source_path`.** When two code pages share a module stem
  (`src/query.rs`, `tests/query.rs`), `ImportResolver` keeps the smaller
  path; when two pages define the same name, neither resolves. Both are
  pure functions of the source-path set, so the edge set is the same in any
  extraction order.

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
(`rewrite::render_fingerprint`) that mixes the entity's content (name,
body, aliases, symbols, imports, summary), the *names* of its linked
neighbors looked up through the entity map (so a neighbor being renamed
re-renders the pages that link to it, while a change to its body does not),
and any preserved `## Notes` text, via the order-sensitive
`hash::combine`. A page is only rewritten to disk
(`rewrite::write_atomic` — temp file + rename, so an interrupted run can't
leave a half-written page) if `cache.needs_render(id, fingerprint)` is true,
i.e. the stored fingerprint for that id differs (or there is none yet).

After rendering, any cache entry whose id is no longer present in the
current entity set is pruned: the corresponding page file is deleted from
`output`, and `cache.retain_ids()` drops it from the cache before it is
saved back to `.wiki/cache.json`. This keeps stale pages from a deleted or
renamed source from lingering in the output directory across incremental
runs. A page file the prune cannot delete (permissions, I/O) is reported as
`warning: could not remove stale page <id>.md: …` on stderr rather than
silently left behind, since `lint` and `serve` would otherwise keep counting
it. Pages the prune cannot account for — ids recorded in a cache written
by an earlier compiler version (`cache::prior_page_ids` reads them past the
version guard) that no current entity produces — are only reported:
`warning: N page(s) from a previous id scheme remain in …` on stderr, never
deleted.

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

**Search** (`Wiki::search`) is case-insensitive, tokenized, with partial
matching:

- The query is lowercased, split on whitespace, each piece's edge
  punctuation trimmed (interior characters like `_` and `:` survive),
  empties dropped, duplicates removed. Empty or punctuation-only queries
  return no hits.
- A page is a hit if any token hits any field. Name, alias, summary and
  section heading hit when every alphanumeric part of the token (`_`, `-`,
  `:` split it) is a prefix of one of the field's words — its maximal
  alphanumeric runs — so `write` no longer hits the title `Rewrite`, `id` no
  longer hits `process-wide`, and `wikilink` still hits `wikilinks`. The
  body hits by substring. Defined-name terms (a top-level definition name
  from `index.json`, lowercased, or one of its snake/CamelCase words; words
  equal to a title word are skipped) and method names (`methods`,
  lowercased whole, never word-split) hit by whole term only, so `to` does
  not hit `token_estimate` and `render` does not hit `needs_render`. Fuller
  matches are not forced above partial ones by a sort key; IDF does that
  where it matters, by making a rare missing token expensive and a common
  one nearly free.
- Score is BM25-shaped, computed in two passes over the pages that pass
  the `kind` filter. Per token: `idf = ln(1 + (N − df + 0.5)/(df + 0.5))`
  over those pages; body term frequency saturates as
  `tf·(k1+1)/(tf + k1·(1 − b + b·len/avglen))` with `k1 = 1.2`,
  `b = 0.75`; the token's contribution is `idf × (name 3.0 + alias 2.0 + defined 2.0 +
  summary 1.5 + heading 1.0 + body 1.0 × tf')`. Method names share the
  `defined` weight. Field weights are not
  length-normalised, and `tf'` stays below the name weight, so a title
  hit beats any volume of body text on the same token. Because `N`,
  `df`, and `avglen` come from the filtered set, `score` is a ranking
  key, not a stable property of a page.
- Hits sort descending by score (`total_cmp`), then descending by
  PageRank as the second sort key, then ascending by id.
- The searched body text is the rendered page's parsed sections minus
  generated chrome (`Metadata`, `Related`, `Referenced By`, `Notes`) —
  subtractive on purpose, so text under a doc's own embedded `## `
  subheadings stays searchable. The heading names of those sections are
  the `heading` field, minus the chrome and the generated content
  headings (`Body`, `Exports`, `Imports`), which sit on nearly every page.
  Occurrence counting and snippet extraction share one lowercased scan
  of the content.
- Hits whose body matched carry a `snippet`: a deterministic excerpt
  around the earliest token occurrence (60 chars of context per side,
  whitespace runs collapsed, `…` on truncated edges). Title/alias/
  summary/heading-only hits have `snippet: None` — the summary explains
  those.

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
   an audio-transcription library), put it behind a Cargo feature rather
   than in the default dependency set, and gate the `reg.register` call on
   the same `#[cfg(feature = ...)]`. Declare the feature when you add the
   extractor, not before: an empty feature is a promise the build cannot
   keep.

**Export-only contract for code extraction:** `CodeExtractor`
(`src/formats/code.rs`) only captures symbols that are part of a language's
public/exported surface — bare `pub` items in Rust (`pub(crate)`,
`pub(super)`, and `pub(in path)` are excluded), exported identifiers in Go,
symbols wrapped in an `export_statement` in JS/TS. Private helpers and
unexported items are deliberately excluded from `Entity::symbols`; a new
language extractor should preserve this contract rather than dumping every
definition in a file.

Rust signatures are **owner-qualified**: a method in an inherent `impl` renders
as `pub fn Wiki::search(…)`, and one in a trait impl as
`fn <TextExtractor as Extractor>::extract(…)` — Rust's own disambiguation
syntax, required because `Display::fmt` and `Debug::fmt` on one type are
otherwise identical strings that `dedup` would merge. Trait impls also emit an
`impl Trait for Type` header, and a `pub` trait declaration emits its required
and default methods. A `pub const`/`static` re-appends its value
(`pub const LIMIT: u32 = 5`) under the 48-char budget described with Python
below. Because tree-sitter queries match at any depth, only
items reachable from the file root through module and type scopes are
captured; an `impl` written inside a function body is not module surface. A
trait impl is gated on neither its own visibility (rustc forbids a modifier
there) nor its target type's, so an `impl Trait for PrivateType` does reach
`## Exports` — the impl patterns capture no visibility and nothing checks
whether the target type is declared `pub`, and resolving that needs name
resolution across files.

**Python signatures are owner-qualified through the full class chain**
(`extract_python.rs`): a nested method renders as `def Article.Inner.deep(self)
-> None`, joined with `.` rather than Rust's `::`. `python_placement` walks
upward from a captured definition and rejects it outright the moment it finds
an enclosing `function_definition` or `lambda` — a **deny-list** of one
excluded node kind, where Rust's `rust_placement` is an **allow-list** of
permitted containers; both walks otherwise keep definitions nested under
`if`/`try`/`with`/`match`/`while`/`for`, since those are genuine module-level
surface (`if TYPE_CHECKING: def …` and `try: def … except ImportError:` are
both kept). Visibility is two gates applied in order: **`__all__`, when the
module declares one authoritatively** — a module-level `__all__` assigned a
literal list/tuple of plain string literals (`python_all`) replaces the
underscore convention for the module-level name (the item's own name when
free, the outermost enclosing class when a member); anything else (a computed
expression, a comprehension, an f-string element) falls back to the
convention, and a later reassignment of `__all__` wins over an earlier one —
and **the underscore convention, always applied inside a class**, since
`__all__` says nothing about what is public *within* a class it lists — even
one it lists by name, a private member stays private. Module
constants and class fields share one query pattern
(`assignment left: (identifier)`) and keep their value up to a 48-`chars()`
budget (`VALUE_BUDGET`; chars, never bytes, to avoid splitting a multi-byte
character): `SUMMARY_LIMIT: int = 300` and `MAX_IDS = 2000` both render
whole. Over budget, an annotated assignment drops the value, since the
annotation still carries the contract; an unannotated one keeps the first 48
chars plus `…`, since it would otherwise say nothing at all. A Rust
`const`/`static` re-appends its value under the same rule as an annotated
assignment: it always has a type, so an over-budget value is omitted, never
truncated. Decorators are kept **with their
arguments** — `@dataclass(frozen=True) class Article` — because a field is
only interpretable through the decorator that governs it.

**Signature normalization is language-agnostic**, unlike the visibility and
owner machinery above: `build_signature` collapses whitespace and tidies the
punctuation a wrapped parameter list leaves behind, for all five languages.
A multi-line Python `def` or Go `func` renders on one line with `( ` and `, )`
cleaned up. This is deliberate; the pass and its limits are documented on
`tidy_punctuation`.

**Rust test-module stripping:** before body assembly,
`strip_rust_test_modules` (`src/formats/extract_rust.rs`) removes each
`#[cfg(test)]`-annotated `mod` item — including the contiguous attribute
run above it — from the Rust source shown in `## Body`, replacing it with
a one-line marker: `// [tests omitted: mod <name>, <N> lines]`. Stripping runs
*before* extraction, so `body`, `symbols`, and `imports` all describe the same
source — an earlier body-only strip left test-only imports such as
`super::Wiki` in `## Imports`. The marker is terminated at a line boundary: it
is a line comment, so code following the module's closing brace on the same
line would otherwise be commented out. `#[cfg(test)]` on a non-`mod` item is
still not stripped.

## Testing

- **Unit tests** live in `#[cfg(test)] mod tests` blocks at the bottom of
  the source file they test (e.g. `src/cache.rs`, `src/hash.rs`,
  `src/lib.rs`, `src/formats/code.rs`, `src/formats/extract_rust.rs`,
  `src/formats/extract_python.rs`) — colocated with the code, not in a
  separate tree. Language-specific extraction tests moved with their
  language when `code.rs` was split (§ Module map).
- **Integration tests** live in `tests/`:
  - `tests/end_to_end.rs` — full `compile()` runs against small in-memory
    corpora: artifact presence, cross-linking, reserved-name remapping,
    and `output_is_deterministic_across_jobs`, which compiles the same
    22-entity corpus (20 `.txt` nodes plus `deep.rs` and `deep_py.py`, so
    the code path is exercised too) with `jobs: Some(1)` and `jobs: Some(8)`
    and asserts the resulting `index.json` bytes and the two code pages are
    identical — the concrete regression test for the determinism rules
    above.
  - `tests/query.rs` — `Wiki::load`, `search`, and `neighbors` against a
    compiled output directory.
  - `tests/cli.rs` — the `wiki` binary's subcommands via `assert_cmd`.
  - `tests/python_extraction.rs` — Python extraction through `compile()`
    on a reduced stand-in for the audit corpora (dataclass, constant,
    private helper, relative imports), asserting on `## Exports`/`## Imports`.
  - `tests/common/mod.rs` — `section`/`exports`/`imports` helpers shared
    by the integration tests, so assertions scope to a curated section
    rather than the whole page (whose `## Body` is verbatim source).
  - `tests/search_quality.rs` — behavioural pins for search ranking over
    in-test fixtures: the 2026-07-14 dogfood checklist queries, and the
    2026-09-02 scoring cycle's partial matching, occurrence monotonicity,
    heading field, and IDF-over-volume cases.
  - `tests/query_list_pages.rs` — `Wiki::list_pages` id/title ordering
    (backs the MCP server's `resources/list`).
  - `tests/mcp_serve.rs` — end-to-end MCP: spawns `wiki serve` and speaks
    raw newline-delimited JSON-RPC over stdio, covering tools and
    resources.
  - `tests/eval.rs` — the retrieval eval harness. Compiles the frozen
    17-page corpus in `tests/.eval_corpus/` and scores 19 labelled queries
    through `Wiki::search`, gating on `top1` and `mrr@10` floors before
    snapshotting a per-case table. Also asserts the `neighbors` pack-size
    ceiling over that real graph (102 packs), which
    `tests/query.rs::max_tokens_is_a_hard_ceiling_on_pack_size` covers only
    on a three-page synthetic fixture.

    **The corpus is frozen at `a55cc54` and must never be re-synced with the
    live files it was copied from** — re-syncing restores the moving target
    the harness exists to remove. It is re-cut only by explicit decision when
    a scoring cycle closes. The directory is dot-prefixed so `walk` prunes it
    whenever ignore rules are respected (`.hidden(respect_ignore)`, and
    `respect_ignore` defaults to true), keeping 17 duplicate pages out of the
    self-hosted wiki, while the harness, which compiles it as its own walk
    root, still sees every file.

    **Every commit that accepts this snapshot must quote the accepted table
    in its message and say which direction each changed aggregate moved.**
    The floors catch a drop in level; the snapshot catches offsetting
    per-case changes that leave the aggregates flat. Neither catches an
    accept-by-reflex, and the commit message is the only thing that does.
    `cargo insta review` needs the `cargo-insta` binary, which is not a
    dev-dependency. **A commit that accepts an improved table must also
    raise `MIN_TOP1`/`MIN_MRR10` to the new exact fractions, in the same
    commit** — the floors are static, so once a cycle raises them, a later
    change can give the gain back with both floors still green, leaving only
    the (directionless) snapshot diff to catch it. Compute the new fractions
    exactly (e.g. `13.0 / 19.0`); never transcribe them from the `{:.4}`
    table, which rounds.
- **Snapshot test:** `tests/snapshot.rs` uses `insta` to pin the exact
  rendered Markdown of one page from a seeded generated corpus
  (`generate_corpus(&input, 12, 42)`), with `source_hash` lines redacted so
  hash-format churn doesn't force a snapshot update for unrelated reasons.
  Snapshots live under `tests/snapshots/`; review changes with
  `cargo insta review` before accepting them.
- **Benchmark:** `benches/pipeline.rs` (criterion) times a full `compile()`
  of a generated 100- and 1000-file corpus (`compile_100`, `compile_1000`);
  run with `cargo bench`.
- **Gate trio** — run all three before sending a change:
  ```bash
  cargo test
  cargo clippy --all-targets -- -D warnings
  cargo fmt --check
  ```
  `.github/workflows/rust.yml` runs the same on every push and pull request
  to `main`, in four steps: `cargo fmt --check`, `cargo clippy --all-targets
  -- -D warnings`, `cargo test`, `cargo doc` with warnings denied. Clippy's
  `pedantic` group is on through `[lints.clippy]` in `Cargo.toml`, with the
  lints that fight this codebase allowed there, each with its reason; `-D
  warnings` turns the rest into failures. Two more workflows: `audit.yml`
  checks `Cargo.lock` against the RustSec database on every lockfile change
  and weekly, and `release.yml` builds Linux and macOS binaries for a `v*`
  tag and attaches them to a GitHub Release with generated notes.
  `.github/dependabot.yml` opens weekly dependency PRs, minor and patch
  bumps grouped into one.
