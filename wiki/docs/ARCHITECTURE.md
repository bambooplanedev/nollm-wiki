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
 rel_path)                                                                                  fingerprints            items.json, llms.txt,
                                                                                                                     AGENTS.md, graph.json
                                                                                                                     (--emit-json)
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
   `disambiguate_ids` runs every colliding group through the
   [disambiguation ladder](#disambiguation-ladder) — directory-segment
   qualification, then a filename discriminator, then a free-id search that
   cannot fail — so no source file is ever dropped for sharing a slug with
   another; a name that is already unique is never touched. `remap_reserved_names`
   then runs, after disambiguation has produced final, already-unique ids: any
   id that happens to equal a reserved manifest base name (`index`, `llms`,
   `agents`, `graph`) is rewritten to `<id>_page`, `<id>_page_2`, ..., with a
   warning on stderr. Finally `extract_cpp::pair_declarations` pairs each C++
   header's declarations with the definitions in its one implementation file:
   the definition inherits the doc and both items' `twin` name the other's
   lines; overloads and ambiguous groups stay unpaired. Produces
   `BTreeMap<String, Entity>`.
4. **Graph + PageRank** (`graph::build_graph`) — builds forward/backward
   links three ways. Mention edges: entity bodies are scanned for other
   entities' names and aliases (the phrase index); a candidate whose target
   is a **code page** is kept only when the body refers to that module in
   code shape (`refers_to_module`: a `stem::`/`::stem` path, a `mod stem;`
   declaration, or the filename `stem.<ext>`, the target's own extension; a
   directory module's stem is its directory, so `formats::` matches and a
   written-out `src/formats/mod.rs` does not), never by prose or a bare
   backticked word — a one-word module name such as `Text` would otherwise
   match every English use of the word. Import edges: every segment of an
   import string that equals a code page's module stem
   (`formats::code::module_stem`, the same rule that names the page) or a
   name in its `defined` list (`ImportResolver`); a `::` path is followed
   only under `crate`, `super`, `self`, or a local crate root, the
   directory holding a `src/lib.rs`/`src/main.rs`, so an import of
   `rmcp::model` never reaches `model`. A C/C++ `#include` is resolved
   differently (`ImportResolver::resolve_include`): it names a path, not a
   sequence of module segments, so segment-wise resolution would draw a
   spurious edge for every path component (`#include "core/book/orderbook.h"`
   hitting pages named `core` and `book` as well as the intended one). The
   bracket/quote-stripped include text is instead matched as a **suffix** of
   a page's `source_path`, anchored to a path-segment boundary, resolving a
   project's own `<mylib/core.h>` while leaving third-party headers
   (`<boost/algorithm/string.hpp>`) unresolved, since no page's path ends in
   them; when more than one page's path ends in the same suffix, the
   lexicographically smallest id wins, for determinism. Link edges: each inline markdown
   link `[text](target)` resolved lexically against the linking file's
   directory to another entity's `source_path` (`resolve_link`; external
   schemes, bare anchors, and targets above the root are ignored). Then it
   runs a fixed-iteration
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
   `graph.json` from the final entity map and graph, and
   `manifest::render_items_json` writes the `items.json` sidecar (before
   `index.json`, whose change triggers a plain `serve`'s reload; with
   `--source`, `serve` reloads after its own compile instead). Items live
   beside the catalog, not in it, because they outweigh it several times over and
   `index.json` is served whole as `wiki://index`. `items.json` is one
   compact object keyed by page id, in id order, holding only code pages
   with at least one item; each value lists that page's items, one object
   per item, keys in this order — `kind` (`fn`, `method`, `type` for a
   struct, class, enum, trait, interface, union, exported macro or type
   alias in every language, `value` for a constant, static or module-level
   variable, `module`), `name`, `owner` (omitted when free), `signature` (as
   the source reads it, no owner spliced in), `start_line`, `end_line`
   (1-based, inclusive, in the original file, from the first attached doc
   or attribute line), `doc` (the doc comment's first sentence, at most 120
   chars; omitted when none), `public` (whether it is in `## Exports`), and
   `twin` (`path:start-end` of a paired C++ declaration or definition;
   omitted when none).
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
| `src/formats/extract_cpp.rs` | The C++ `LangSpec` and `cpp_shape`: owner qualification through class/namespace scopes via `::` (replacing, not prepending to, a qualifier an out-of-line definition already wrote), template-head retention, no visibility gate (a header's declared surface is extracted regardless of `public`/`private`), and the two-pages-per-component `.h`/`.cpp` pairing (`source_rank`, `module_stem`). |
| `src/formats/extract_simple.rs` | JS, TS, and Go — three specs with no owner resolution, gated by `export_statement` or (Go) a leading-capital naming convention. |
| `src/formats/extract_jsx.rs` | JSX and TSX — the JS and TS specs reused unchanged, TSX on the grammar that accepts JSX in expressions; `index.jsx`/`index.tsx` are directory modules. |
| `src/formats/extract_vue.rs` | Vue SFCs — top-level `<script>`/`<script setup>` blocks go to the JS/TS/JSX/TSX spec their `lang` names; `<template>`, `<style>` and custom blocks are blanked in place so every byte offset still points into the file. Holds the block scanner `extract_svelte` reuses. |
| `src/formats/extract_svelte.rs` | Svelte components — the same blank-in-place view over the instance and `context="module"`/`module` scripts; every top-level element but `<style>` is nested markup (so a `<script>` in `<svelte:head>` is not the component's), void elements and `{...}` expressions are skipped. |
| `src/formats/summary.rs` | `summarize()` — deterministic, no-LLM one-line summary via a fallback chain (front-matter desc → docstring → first sentence of body → first signature). The sentence is cut from the whole opening paragraph (consecutive non-empty lines joined by a space), so a sentence wrapped at 80 columns in a `//!` block or README prose is not truncated at the line break. A sentence ends only at `.`/`!`/`?` followed by whitespace or end of line (so `index.json` does not end one), and lines opening "This document/page/module/file/note" are skipped as boilerplate. |
| `src/model.rs` | Core types: `Entity`, `Edges`, `Graph`, `LintReport`, `SourceKind`, `KindFilter` (the `code`/`docs`/exact-kind search filter), `CodeItem` (with the `test` flag) and `is_test_path()`; `title_case()` — the one casing rule every name-deriving path shares; `slugify()` — folds a name to an id matching `[a-z0-9_]+` in a single ASCII-fold pass (lowercase, alphanumeric kept, any run of other characters collapsed to one `_`), falling back to an anonymous `page_<hash>` id if nothing alphanumeric survives the fold; `normalize_path()`. |
| `src/graph.rs` | `build_graph()` — mention edges (phrase index, filtered by `refers_to_module` for code targets), import edges (`ImportResolver`: stem and `defined`-name segments under local roots), markdown-link edges (`resolve_link`), and PageRank; `orphan_ids()`. |
| `src/hash.rs` | BLAKE3-based `hash_bytes`/`hash_str`/`combine`/`to_hex` used for content hashes and render fingerprints. |
| `src/rewrite.rs` | Page rendering (`render_page`), fingerprinting (`render_fingerprint`), `## Notes` section preservation, atomic file writes (`write_atomic`); code-block masking, byte-offset preserving: `mask_fenced_code` backs `parse_sections` (so a `##` heading quoted in a fence never becomes a section, for rendering and search alike), and `mask_code` adds `mask_inline_code` on top for lint's link scan. |
| `src/cache.rs` | `.wiki/cache.json` — versioned incremental-render cache (`Cache`, `load`, `save`). |
| `src/manifest.rs` | Builds `Manifest` and renders `index.json`, `items.json`, `index.md`, `llms.txt` (pages at or above the median PageRank under `## Docs`, the rest under `## Optional`), `AGENTS.md`, `graph.json`. |
| `src/lint.rs` | `lint()` — in-memory broken-link and orphan-page checks over rendered pages. |
| `src/query.rs` | `Wiki` — loads a compiled output dir (`index.json`, `items.json`) for `search()` (page hits with symbol lines) and `neighbors()` (context-pack) queries, with `resolve_id()` mapping an id, title or path to a page, plus `page()`/`has_page()`/`list_pages()` for the MCP resources; used by the CLI, the MCP server, and as a library API. See [Query internals](#query-internals). |
| `src/outline.rs` | `Wiki::outline` / `Wiki::page_text` — what `wiki page` and the MCP `page` tool print: a header, imports and one line per `items.json` item (`start-end  kind  signature`), or a markdown page's headings. Under `max_tokens` it cuts test items, docs and imports in that order, then trailing items, and ends with a `… dropped: … — pass max_tokens: N for all` marker. `page_text` with `full` returns the page, or its outline / a whole-line prefix when over budget. See [Query internals](#query-internals). |
| `src/serve.rs` | `wiki serve` — MCP server over stdio (`rmcp`): `search` (`format`: `lines`/`json`; `search_lines`, `search_json`)/`neighbors`/`page`/`lint` tools and `wiki://page/<id>`, `wiki://index`, `wiki://llms.txt` resources. Without `--source`, `WikiState` lazily reloads the compiled wiki when `index.json`'s fingerprint (mtime, len) changes; with it, `WikiState` takes snapshots from `Live`. A failed reload keeps the last good snapshot; stale snapshots add a `wiki may be stale: <reason>` line to tool results and resource reads. |
| `src/live.rs` | `Live` — `serve --source`: watches the project, recompiles in a background thread until the compiled generation catches up with the event generation, and holds the current `Wiki` snapshot plus the stale reason. Each tool call writes a barrier file in `<dir>/.serve/`, waits for its event, then waits for the compile (both within one 5 s bound). |
| `src/generator.rs` | `generate_corpus()` — deterministic synthetic-corpus generator (SplitMix64 PRNG) used by `wiki generate` and tests. |
| `src/watch.rs` | `watch()`/`recompile_once()` — filesystem-watch-triggered recompilation, ignoring reads (`Access` events: inotify reports every open, the compile's own walk included), events under `output`, and paths the compile's ignore rules skip (`WatchSet`), debounced 150 ms; prints `watching … (Ctrl-C to stop)` and `recompiled: N pages (M written)` on stderr. Also supplies the watch setup (with an event hook) and `WatchSet` that `live.rs` reuses. |
| `src/main.rs` | CLI entry point (`clap`): `compile` (with `--watch` to loop via `watch::watch`), `neighbors`, `page` (`--full`, `--max-tokens`), `search` (`--kind`, `--format lines|json`), `lint`, `serve`, `generate`. |

**The `extract_*.rs` names are two words on purpose, not tidiness.** Page
titles are derived from file stems and are inputs to both the graph and
search. When these names were chosen, a mention edge needed only the
title's words in prose, so a page titled `Rust` (from a hypothetical
`rust.rs`) drew an edge from every prose mention of "rust" — measured at 9
files for `Rust`, 2 for `Python`, 1 for `Simple`, on this repo alone — and
`code_rust.rs` failed too, because the compiler's own `SourceKind` string
`code:rust` tokenizes to the adjacent words `code` and `rust`. That graph
cost is gone: a mention links a code page only in code shape (stage 4
above). The names stay two words for search. Field weights are not
length-normalised, so a bare `rust` scores the same on `Rust` as on `Extract
Rust`; what the second word buys is the compound query — `extract rust`
collects the name weight on both words for `Extract Rust` and on one for
`Rust` — and a title that says what the page is about (extraction) rather than
only what it is about something (Rust).

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
still matters for search, where a compound query such as `extract rust`
scores the name field on both words.

## Disambiguation ladder

`disambiguate_ids` (`src/lib.rs`) gives every entity a unique id. An
extractor names a page from its own file in isolation — it cannot see
whether some other file in the tree would slugify to the same id — so this
runs after every entity is extracted, over every group of entities that
currently share a slug. Each group escalates through three rungs, in
increasing order of how much the rendered name has to change, and a group
stops climbing as soon as a rung actually separates its members:

1. **Directory-segment qualification.** Prefix one more parent-directory
   segment onto every member's name, one round at a time, until the group's
   directories diverge or a member runs out of path to take
   (`app/api/models.py` + `app/db/models.py` → `Api Models` / `Db Models`,
   both kept). A name that is already unique is never touched — qualification
   is collision-driven, so the short titles the wiki is readable with survive
   and a qualified title is always evidence of a real clash.
2. **Filename discriminator.** If directory prefixing cannot separate the
   group (same directory, or every member already out of segments), append a
   discriminator built from the filename itself — once — to every member
   except the group's unique minimum by `(source_rank, dot count)` (its
   canonical member, when the minimum isn't tied). For a code page named
   after its own module stem, the discriminator is the dotted segments
   between the stem and the final extension (`client.test.ts` → `Client
   Test`), or, when there is no such infix, the extension itself
   (`orderbook.h` / `orderbook.cpp` → `Orderbook H` / `Orderbook Cpp` — the
   shape a C/C++ header and its implementation file collide in, since both
   share one `module_stem`). A discriminator that would just double a token
   already in the name (`discriminator_is_redundant`) is skipped rather than
   applied.
3. **Free-id search.** Whatever still collides after both rungs above —
   two files at the project root with no segment left to take, two paths
   equal but for case, or any other residual clash — is resolved by visiting
   the group in `source_path` order and searching ordinals `0, 2, 3, 4, …`
   for the smallest one whose rendered id is not already claimed this round.
   This is the rung that **cannot fail**: the free-id space is unbounded, so
   a free id always exists, and no source file is ever skipped or dropped
   for colliding with another — the property this branch exists to
   guarantee. It is not a plain position → ordinal mapping: a member's own
   root name can independently already render an ordinal-shaped id
   (`docs/2.md` titled "Chapter 2" sitting beside a colliding
   `docs/chapter.md` / `docs/Chapter.md`), so which numbers are actually free
   can only be answered by checking every id claimed so far, not by counting
   the group's own members.

Termination is bounded on every rung — depth by the path's segment count,
the discriminator by its once-only flag, and the free-id search because each
round searches fresh from the round's claimed set rather than incrementing a
remembered value. A belt-and-braces round cap in `disambiguate_ids` forces
the last possible round straight to the free-id search and stops, in case a
future change ever breaks one of those per-rung guarantees; on every input
observed so far the ladder resolves well before that cap is reached.

**`remap_reserved_names` runs after this ladder, not before it, and over its
already-final ids.** It only rewrites an id that happens to equal a reserved
manifest base name (`index`, `llms`, `agents`, `graph`) — appending `_page`,
then `_page_2`, ... until the rewritten id is free. This ordering is what
makes a C/C++ pair land the way it does: `graph.h` and `graph.cpp` share the
module stem `graph`, so rung 2 gives the `.cpp` file (the higher
`source_rank`) the discriminator `graph_cpp` and leaves the header, which
wins the tie-break, holding the bare id `graph`. Only then does
`remap_reserved_names` see that `graph` collides with the reserved manifest
name `graph` (`graph.json`) and rewrite the header's id to `graph_page` —
while `graph_cpp` was never reserved and is left untouched. Running the
remap first would see no `.h`/`.cpp` collision at all yet (both still named
`graph` at that point) and remap the wrong thing.

Renaming a page also changes what `graph::build_graph` matches against body
text: a qualified two-token name draws fewer lexical mention edges than the
single word it replaced. That is the trade against a page that, before this
ladder existed, would not have been written at all.

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
  `disambiguate_ids` (see [Disambiguation ladder](#disambiguation-ladder))
  reads only each entity's `(name, source_path)` pair and extends every
  member of a colliding group together, so the ids it produces are the same
  whichever order extraction finished in. Every rung terminates on its own —
  directory-segment prefixing because a path has finitely many segments, the
  filename discriminator because it applies at most once per entity, and the
  free-id search because each round searches fresh from that round's claimed
  set rather than incrementing a remembered value — and the group only
  reaches the free-id rung by visiting its members in sorted `source_path`
  order, so the ids it lands on do not depend on extraction completion
  order.
- **The free-id search assigns every remaining colliding entity a page,
  never drops one.** Where an earlier scheme kept only the
  lexicographically-first `rel_path` and skipped the rest, the ladder's last
  rung searches an unbounded ordinal space instead, so a group with three
  colliding members after qualification and discrimination gets three
  distinct ids, not one survivor. Combined with the sorted-walk guarantee
  above and the fixed `source_path` visiting order, which id lands on which
  ordinal is a pure function of the input paths, not of extraction
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
  pure functions of the source-path and defined-name sets, so the edge set
  is the same in any extraction order.

## Incremental build

With `--incremental`, `compile()` loads `.wiki/cache.json` (via
`cache::load`) instead of starting from an empty cache. **The flag gates the
load, not the save:** every compile writes `.wiki/cache.json` back
(`cache::save`), because that file is also the only record of which pages
this compiler wrote, and the prune below is only as effective as that record
— gating the save too meant a workflow that never passed `--incremental`
never wrote a cache, so `prior_page_ids` was always empty, `cache.pages`
held exactly the live ids the render loop had just set, and the prune,
though it ran on every compile, had nothing to prune. The cache is
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

**Pruning runs on every compile, incremental or not** — `--incremental`
only decides whether the render cache is trusted to skip re-rendering an
unchanged page, not whether `output` may go on holding files that no
current source produces. After rendering, `prune_stale` (`src/lib.rs`)
deletes every page file the compiler previously wrote (per the cache, plus
any ids left over from a prior id scheme — see below) whose id is no longer
in the current entity set, and `cache.retain_ids()` drops the same ids from
the cache before it is saved back to `.wiki/cache.json`. This keeps stale
pages from a deleted or renamed source from lingering in the output
directory. A page file the prune cannot delete (permissions, I/O) is
reported as `warning: could not remove stale page <id>.md: …` on stderr
rather than silently left behind, since `lint` and `serve` would otherwise
keep counting it.

**The prune has a safety net: it never deletes a page carrying
hand-written notes that were not migrated onto a live page.** Before
deleting a stale `<id>.md`, `prune_stale` first checks whether the file is
in `consumed` — the set of prior page files whose `source_path` a current
entity still owns (see below), i.e. this compile already migrated that
exact file's notes onto a live page, possibly under a new id. If it is not
consumed, `prune_stale` falls back to checking the file's own `## Notes`
section (`rewrite::read_notes_from`) directly: a non-empty one means real,
unmigrated notes, and the page is kept with `warning: keeping <path> — it
carries hand-written notes that were not migrated` on stderr, rather than
silently discarding someone's notes because the source that once produced
that id disappeared or renamed to something else. Pages the prune cannot
otherwise account for — ids recorded in a cache written by an earlier
compiler version (`cache::prior_page_ids` reads them past the version
guard) that no current entity produces — get the same diagnostic treatment:
`warning: N page(s) from a previous id scheme remain in …` on stderr, never
deleted.

**Notes preservation across a rename:** a page's *identity*, for notes
purposes, is its `source_path`, not its id — an id can change under
[disambiguation](#disambiguation-ladder) or a source-file rename, and
`<new_id>.md` would not exist yet even though the same source's notes do.
`rewrite::page_source_index(output)` scans every already-written page,
reads its `## Metadata` block, and maps `- source: <path>` back to **every**
file that declared it. Before rendering, each entity looks itself up in that
index by its own `source_path`: if a prior page is found, its `## Notes`
section (`rewrite::read_notes_from`) is read and passed into `render_page`,
re-emitted verbatim in the new version of the page — so hand-written notes
under a page's `## Notes` heading survive a plain re-render, a rename, or
an incremental skip, as long as the heading itself isn't renamed. The same
`source_path → prior files` index is what lets the prune above tell "this
file's notes were migrated onto the entity's new id" apart from "this
file's source is genuinely gone."

**One source can be declared by several page files, and the entity's own
`<id>.md` always wins.** An orphan left by an earlier id scheme goes on
declaring the live source until a prune removes it, so the mapping is
one-to-many and `rewrite::prior_page_for` chooses: the entity's own page
file if it is among the candidates, otherwise the lexicographically
smallest — the genuine rename case, where `<new_id>.md` does not exist yet.
Picking the smallest unconditionally, as an earlier revision did, let an
orphan whose name happened to sort first hijack a live page: its notes were
read out of the dead file and overwritten, and the orphan's stale notes
transplanted onto the live page in their place. `consumed` is built through
the same rule, so a candidate that lost is *not* recorded as migrated and
still reaches the prune's own `## Notes` check.

The ordering here is load-bearing: the index is built before anything is
written, and every notes read happens inside the parallel render map, whose
`.collect()` completes before the write loop begins — so no notes read can
observe a page this run wrote.

## Query internals

`Wiki::load` (`src/query.rs`) reads `index.json` — metadata plus adjacency —
and `items.json` (each code page's items, absent for older wikis); page
bodies are read on demand from `<id>.md`. From the items it builds the
per-page symbol fields below.

**Id resolution** (`Wiki::resolve_id`, used by `neighbors` and `page` in the
CLI and MCP) tries, in order, and takes the first that names exactly one page:
the exact id; the title (case-insensitive); the slug of the argument; the
argument normalised (`\` to `/`, one leading `./` stripped) as an exact page
path; a unique page path ending in `/<argument>` (several are an error listing
them); for an absolute path, the longest page path that is a `/`-suffix of the
argument. Otherwise the error lists up to five ids sharing a word with the
argument.

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
  `defined` weight. Code items add three more page fields: the whole item
  name, also as `owner::name` (`W_SYMBOL` 2.8, in place of `defined` and
  below the title weight), the words of item names (`W_SYMBOL_WORD` 2.0,
  methods `W_METHOD_WORD` 2.0), and the words of item doc sentences
  (`W_SYMBOL_DOC` 0.75). Test items (`CodeItem.test`) and module items
  feed none of them, nor the `defined` and method names that only test
  items carry, and a word that is already a title word or defined name is
  skipped. Field weights are not length-normalised, and `tf'` stays below the name weight, so a title
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
  whitespace runs collapsed, `…` on truncated edges, comment markers such
  as `//`, `///`, `*` and `#` dropped when they stand as whole tokens).
  Title/alias/summary/heading-only hits have `snippet: None` — the summary
  explains those.
- **Symbol expansion** (`expand`) runs after the sort and `limit` cut, on
  the surviving code hits only. Each item of a hit page (modules excluded)
  scores per token `idf × (name + doc)`: whole name or `owner::name` 3.5
  (`IW_WHOLE`), a name word 2.0, a doc word 1.0; test items are scaled by 0.5
  (`TEST_ITEM_FACTOR`). A whole-name match outranks any word match even
  with a doc match (a compile-time assert pins it). Per page the top 3
  (`SYMBOLS_PER_PAGE`) distinct items are kept, ties by line; across the
  response a line must score at least 0.3 (`MIN_REL_SYMBOL`) of the best
  line. Page score and order are not changed by expansion. A `SymbolLine`
  carries `path`, the 1-based inclusive line range, `owner::name`, the
  signature, and the doc, which is dropped when an earlier line of the
  same page shows the same one. C++ gives each declarator of a shared
  declaration its own signature, which therefore may not be verbatim in
  the file.
- **The test flag.** `CodeItem.test` is set at extraction (`formats/code.rs`):
  the file is a test path (`model::is_test_path`: a `tests` path component,
  or a file named `test_*`, `*_test.*`, `*_tests.*`), or, for Rust, the item
  sits in a `#[cfg(test)]` module or carries `#[test]` (also
  `#[tokio::test]`). It is stored in `items.json`. Test items stay visible
  as symbol lines but contribute no page fields, and a `defined` or method
  name only test items carry is dropped from the page's defined terms
  (`Wiki::load`), so their names cannot lift their page; body text still
  counts.
- **Kind filter.** `KindFilter` accepts `code` (any `code:<lang>`), `docs`
  (`markdown` and `text`) or an exact kind; the MCP `kind` and CLI `--kind`
  share it and its error text.
- **Output.** `serve::search_lines` renders the default line format (page
  line `id  path  title — summary`, indented symbol lines
  `path:start-end  name  signature  — doc`, or the snippet for a page
  without symbols; `no matches` when nothing hit); `serve::search_json`
  renders the JSON form. The MCP `format` parameter and the CLI
  `--format` choose between them; `lines` is the default for both.

**Neighbors** (`Wiki::neighbors`) takes a page id; both the CLI and the
MCP tool first pass what the caller typed through `Wiki::resolve_id`, which
accepts an exact id, a title (ignoring case), the slug of the argument, or
the one page whose source path equals it or ends with `/<arg>`. A failure
lists up to 5 closest ids (the pages an ambiguous path matched, else ids
or titles sharing a word). It BFS-collects ids to `depth` hops over
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

**C++ has no visibility gate at all** (`extract_cpp.rs`): every declaration
tree-sitter's query reaches is extracted, `public`, `protected`, and
`private` alike. Gating on `access_specifier` would be inconsistent — an
out-of-line `void OrderBook::rebuild_()` carries no visible specifier at its
definition site and would be kept regardless — and a sticky
previous-sibling scan would default a member nested in a `template_declaration`
or a `preproc_ifdef` to private and drop it, since neither has a sibling
specifier in its enclosing `field_declaration_list`. **Owner qualification
uses `::`, and REPLACES an existing qualifier rather than prepending to
it**, the one respect in which C++ departs from Rust's and Python's
owner-splicing: an out-of-line definition already writes its own qualifier
in the source (`void OrderBook::apply(...)`), so anchoring the splice at the
name, as every other language does, would insert a second one ahead of it
(`void OrderBook::OrderBook::apply()`); anchoring at the outermost
`qualified_identifier` instead makes the resolved owner replace it, which
also strips a namespace the source wrote (`void trading::OrderBook::apply()`
renders `void OrderBook::apply()`). A `template<...>` head is retained via
`cpp_sig_start`, climbing through nested `template_declaration` parents the
same way Python's `decorated_definition` climb keeps a decorator with its
class. **A header and its implementation file are two separate pages that
share one identity for linking purposes**: `module_stem` gives
`orderbook.h` and `orderbook.cpp` the same stem, `source_rank` ranks a
`.h`/`.hpp`/`.hh`/`.hxx` header below its `.cpp`/`.cc`/`.cxx` implementation,
and `ImportResolver` uses that rank so a bare `#include "orderbook.h"` or a
`::`-free reference to `OrderBook` resolves to the header — the file that
actually *declares* the public surface — even where the two files' own
disambiguated ids differ (see [Disambiguation ladder](#disambiguation-ladder)).
A module-level `///`/`//!`/`/**`/`/*!` Doxygen comment, or one continuing
with a bare `*` while genuinely still inside such a block, supplies the
module summary the same way a Rust `//!` or Python module docstring does;
`leading_doc` is also allowed to skip forward past an actual license/SPDX
banner in search of the real doc beneath it, C++ only, since that banner
shape is common at the top of a C++ file and every other language keeps the
stricter rule that a doc comment must open the file or there is none.

**One part of that restructuring is NOT C++-only: a blank doc line no longer
ends the paragraph, in any of the six languages.** `leading_doc` used to stop
at the first line that stripped to `""`; it now skips it and keeps
collecting, because a bare `/**`, a lone `*/` and a `//!` with nothing after
it are all gaps *within* one doc comment, not its end. So a Rust module doc
of `//! Widget registry`, a blank `//!`, then `//! Detail paragraph here.`
summarises as `"Widget registry Detail paragraph here."` where it once gave
`"Widget registry"` — the whole leading run of doc lines reaches
`summarize()`, which then cuts the first sentence out of it.
`a_blank_doc_line_joins_the_paragraph_on_either_side_of_it` pins this for
Rust, and `a_doc_comment_must_open_on_the_first_line_for_every_non_cpp_language`
pins that the banner-skip did *not* travel with it.

**Signature normalization is language-agnostic**, unlike the visibility and
owner machinery above: `build_signature` collapses whitespace and tidies the
punctuation a wrapped parameter list leaves behind, for all six languages.
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

**Items are every top-level item and method, not the export surface.** A
language's query marks definitions that may be private with `@item` beside
its `@def` patterns; a match yields an item from either, deduplicated on
the `@name` node so a `pub fn` matched by both is one item. `public` is the
`@def` match's `should_keep` result, so it agrees with `## Exports` exactly;
an `@item`-only match is private where the `@def` patterns already gate
visibility (Rust, JS/TS), and runs `should_keep` itself elsewhere. Covered:
functions and methods, types (`type` aliases included, in every language),
Rust `mod`s, and top-level values — Rust `const`/`static`, Python module
assignments, C++ namespace-scope variables, Go package-level `const`/`var`
(one item per name; a spec in a `const ( … )` group has its own lines and
doc), and JS/TS top-level variables that hold no function. C++, Go and JS/TS
values are `@item`-only, so `## Exports`, `defined` and `methods` never see
them; Rust `const`/`static` and Python module assignments are `@def`s.
Fields, variants, associated consts and types, impl headers and
function-local definitions are not items. An item's range climbs from its
signature node over the contiguous leads directly above it
(`LangSpec::item_lead`: Rust attributes and `///` lines, Doxygen comments,
JSDoc blocks, Go `//` lines, with Go directives such as `//go:generate`
kept in the range but out of the doc), stopping at a blank line, at a lead
that trails code on its own line (`int n; ///< Count.`), or at anything
else, so a stray doc comment never stretches it. A C++ member-trailing doc
(`///<`, `//!<`, `/**<`, `/*!<`) stops the climb even on a line of its own:
it documents the member before it.
Items of a Rust file with a test module come from a second pass over the
**unstripped** text and its tree (`Stripped::Rewritten` carries it), so
their lines are the file's and test functions are items too, while
`## Exports` and the body still come from the stripped text. Such an item
stays `public` only if the stripped pass has it as public too, so a bare
`pub fn` inside the test module reads private, as `## Exports` has it.

Known gaps: a private `macro_rules!` is not an item (`rust_placement`
rejects it before visibility is known), nor is a function in a Rust
`extern` block; a JS/TS method of an exported class, or an exported
variable, reads `public: false` because no export pattern matches it; an
object-literal or anonymous-class method is a free `fn`, its owner
unknown; and the names of one multi-declarator statement share its range.

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
  - `tests/cpp_extraction.rs` — C++ through `compile()`: a header/impl pair
    both exist as pages and their shared `#include` draws exactly one edge,
    a third-party angle-bracket include (`<vector>`) draws none, a
    project-local angle include still resolves, and a free function
    declared in the header and defined in the `.cpp` — both pages
    legitimately `defined`-ing the same name — still resolves by name
    rather than going dead the way a genuine two-definition clash does.
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
- **Symbol eval:** `tests/symbol_eval.rs` — the symbol-level retrieval
  harness. Compiles the frozen corpus in `tests/.symbol_corpus/` (this
  crate's own `src/`, `docs/ARCHITECTURE.md` and `README.md`) and scores 52
  labelled queries (Name, NameWords, DocOnly, Stopwords, MethodSplit, V7,
  TestTarget) through `Wiki::search`. A case names the expected
  `(path, symbol)` and is scored twice: `file_rank` is the first hit on the
  expected path, `symbol_rank` the position of the expected symbol among the
  response's symbol lines in output order. Per split (`tune`/`held`) it
  reports `file_hit@1`, `file_hit@3`, `mrr@10`, `symbol_hit@3` (symbol rank
  at most 3), and mean response tokens: `tokens` charges the default
  response (`search_lines`, limit 10), `tokens_json` the `format: "json"`
  response (`search_json`). Gates, checked before the snapshot: `file_hit@3`
  and `symbol_hit@3` floors per split (tune 35/35, held 16/17) and the
  title-as-query count (27/28). The `neighbors` pack size is recorded
  without being asserted. A step that changes the default renderer or limit
  swaps it in the harness and quotes both token numbers.

  **The corpus is frozen on 2026-10-22 at `a81a332` (the crate's sources as of
  `7209ee2`; the SHAs may dangle after a squash, `SYMBOL_CORPUS_HASH` is the
  real pin) and must never be re-synced with the live files**; it is re-cut only by
  explicit decision, in a commit quoting the old and new tables. **Held-out
  rule:** every third case per class is `held_out`, pinned by a test, and is
  never used to choose constants.
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
