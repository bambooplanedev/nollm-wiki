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
wiki-code/src/walk.rs src/formats/            src/lib.rs             src/graph.rs         src/rewrite.rs          src/manifest.rs         src/lint.rs
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
   `Vec<SourceFile>`. Memory folders (`.agents/memory` in the input, plus
   each of `CompileOptions.memory`, a list) are walked separately by
   `walk::walk_memory`: ignore rules off, hidden files, `MEMORY.md` and a
   top-level `logs/` folder skipped, markdown and text only; their pages carry `memory: true` and, for an external folder,
   absolute `/` paths.
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
   warning on stderr. Finally `wiki_code::pair_declarations` pairs each C++
   header's declarations with the definitions in its same-stem implementation
   file (only its body-less declarations and guarded definitions; an
   out-of-line `= default` counts as a definition; `= delete` and in-class
   `= default` count toward no key), else, for a body-less
   declaration, with the one guarded definition (out of line, not `static`,
   not in an anonymous namespace) of a key unique across the project: the
   definition inherits the doc and both items' `twin` name the other's lines;
   overloads of one parameter count pair by their parameter types, spelled
   alike and in one namespace, and ambiguous keys stay unpaired. Produces
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
   name in its `defined` list (`wiki_code::imports::Imports`); a `::` path is followed
   only under `crate`, `super`, `self`, or a local crate root, the
   directory holding a `src/lib.rs`/`src/main.rs`, so an import of
   `rmcp::model` never reaches `model`. A C/C++ `#include` is resolved
   differently (`wiki_code::imports::Imports::resolve_include`): it names a path, not a
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
   Then `calls::resolve_calls` turns each function item's call sites
   (gathered at extraction: every `call_expression`/`call` in its lines)
   into `calls`, the `path:start-end` of the project functions they name,
   matched by name, and by argument count where the parameter list proves
   a mismatch (`Arity`), within the caller's language (the JavaScript
   dialects and components count as one): `f()` is the caller's owner's
   method, else a free function in its file or a file it sees (its imports
   and theirs), a method a file it sees (a C++ base class's), or the
   project's only free function of that name;
   `Q::f()` and `x.f()` on a project type or module are that owner's or
   module's; other `x.f()` are the methods the caller's file sees, else the
   project's only one. A `Q` that is no project type or module is outside
   the project. A header declaration stands for its definition: its
   `twin`, else the one same-named function of its owner in a file of the
   same stem.
5. **Render** (`rewrite::render_page`, in parallel) — for each entity,
   reads any existing page to preserve its `## Notes` section, computes a
   content fingerprint (`rewrite::render_fingerprint`), and renders the
   page body. A text body's own links go through `rewrite::obsidian_fixups`
   (also applied to `index.md` summaries; prose only, never code): `[[x]]`
   whose `slugify(x)` is a page becomes `[[id]]`, a relative Markdown link to
   an indexed page becomes `[text](id.md)`, and a `#` Obsidian would read as
   a digit-led tag is escaped. Cross-links are emitted as slug-target, Obsidian-flavored
   wikilinks — `[[slug|Name]]` (e.g. `[[gradient_descent|Gradient
   Descent]]`) — so each link target is a real `<slug>.md` file rather than
   a display name that has to be re-slugified to resolve. A page opens with
   YAML front matter (`created`, `updated`, `aliases`, `tags` led by a
   `wiki/code`, `wiki/doc` or `wiki/memory` kind tag, `kind`, `source`,
   `source_hash`; readers also accept the `## Metadata` bullets of 0.6.7 and
   earlier), and a code page's `## Body` is a fenced block tagged with the
   language; search drops those two fence lines (`query::unfence`) so scoring
   and snippets see the bare source. Writes only land
   on disk if not in incremental mode, or if the fingerprint changed (see
   [Incremental build](#incremental-build)).
6. **Manifest** (`manifest::build_manifest`) — builds `index.json`,
   `index.md`, `llms.txt`, `AGENTS.md`, and (with `--emit-json`)
   `graph.json` from the final entity map and graph, and
   `manifest::render_items_json` writes the `items.json` sidecar (before
   `index.json`, whose change triggers a plain `serve`'s reload; with
   `--source`, `serve` reloads after its own compile instead). Items live
   beside the catalog, not in it, because they outweigh it several times over and
   `index.json` is read whole. `items.json` is one
   compact object keyed by page id, in id order, holding only code pages
   with at least one item; each value lists that page's items, one object
   per item, keys in this order — `kind` (`fn`, `method`, `type` for a
   struct, class, enum, trait, interface, union, exported macro or type
   alias in every language, `value` for a constant, static or module-level
   variable, `module`, `top_level` for a shell file's calls outside any
   function), `name`, `owner` (omitted when free), `signature` (as
   the source reads it, no owner spliced in), `start_line`, `end_line`
   (1-based, inclusive, in the original file, from the first attached doc
   or attribute line), `doc` (the doc comment's first sentence, at most 120
   chars; omitted when none), `doc_text` (the whole doc on one line, at most
   1000 chars; omitted when it equals `doc`), `public` (whether it is in `## Exports`), and
   `twin` (`path:start-end` of a paired C++ declaration or definition;
   omitted when none), `test` (omitted when false), `calls` (the
   `path:start-end` of each function a function calls, sorted; omitted when
   none) and `call_lines` (per `calls` entry, the lines that call it).
7. **Lint** (`lint::lint`) — checks the in-memory rendered `pages` map for
   broken `[[links]]`, orphaned pages and non-code pages too large for a default
   `neighbors` pack (`large`: id and tokens); no disk re-read. Links are scanned
   on `rewrite::mask_code` output, so a `[[link]]` inside a fenced block or
   inline code span is never reported, and a code page's `## Body` (verbatim
   source, per its front-matter `kind`) is dropped from the scan altogether.
   The CLI exits 1 on any broken link; orphans are advisory.

## Module map

The workspace holds five crates. `wiki` (`crates/wiki`) owns
everything about a wiki: the pipeline, pages and their ids, the graph,
rendering, the manifest, queries, the CLI, `serve` and `setup`. `wiki-code`
(`crates/wiki-code`, lib `wiki_code`) owns the code layer a sibling tool can use
without the wiki: the item model, the source tree walk, tree-sitter extraction
(`extract`, and `analyze`, which also hands back the parse trees, each item's
node, the calls under a node with their literal text, and comment text), the
candidate index call sites resolve against, and import resolution. Its crate
doc lists that contract; its `support` module is shared text helpers and not
part of it. It pins tree-sitter and every grammar to an exact version, since a
consumer that depends on it by git does not use this lock file. `techlog`
(`crates/techlog`) holds where agent memory lives, and the `Origin` of a
memory note. `plaz` (`crates/plaz`) is the second tool of the set: it draws per-function decision charts and call
targets from the code layer in `wiki-code`, depends on nothing of `wiki`, and writes its pages to `.nollm-wiki/map/`, which
`compile` leaves alone. `nollm-wiki-install` (`crates/nollm-wiki-install`) is the installer of the set: it installs, updates and
removes the modules' release binaries in `~/.nollm-wiki-home` and runs each module's own `setup`; it depends on no other
crate of the workspace, and `install.sh` at the repository root fetches it. Paths below that start with `src/` or `tests/` are in
`crates/wiki`.

| Module | Responsibility |
|---|---|
| `src/lib.rs` | `compile()` / `compile_inner()` — orchestrates the pipeline; slug dedup; `CompileOptions`/`CompileResult`/`WikiError`; with `WIKI_TIMINGS=1`, one stderr line of stage timings (`walk`, `extract`, `global`, `render`, `write`, `tail`). |
| `src/ids.rs` | Page-id naming: `disambiguate_ids` name qualification (the collision ladder that makes every page id unique) and the reserved-name remap (`remap_reserved_names`). |
| `crates/techlog/src/lib.rs` (crate `techlog`, re-exported as `memory`) | Where agent memory lives: `REPO_DIR`; Claude Code's folder for a project (`claude_slug`: every UTF-16 unit that is not an ASCII letter or digit becomes `-`, cut at 200 with a Java-style hash; `claude_memory_dir`: `<config dir>/projects/<slug>/memory`, config dir `CLAUDE_CONFIG_DIR` else `$HOME` (then `USERPROFILE`)`/.claude`, keyed on the main checkout via `git rev-parse --git-common-dir`); `agent_dirs` (one entry per adapter, Claude Code only for now); `resolve` and `display_path` for stable external paths. `Origin` (a note's origin: `local`, or `repo` with its last author and whether it was edited; `trust` ranks them) and `repo_origins`, which reads it from git, live here too; `wiki` reaches them as `memory::Origin`. |
| `crates/wiki-code/src/walk.rs` (re-exported by `src/walk.rs`) | Filesystem walk with `.gitignore`-aware filtering that also prunes `SKIPPED_DIRS` (process folders and vendored code); produces sorted `Vec<SourceFile>`. |
| `src/formats/mod.rs` | `Extractor` trait, `Registry` (extension → extractor dispatch), `ExtractError`. |
| `src/formats/text.rs` | `TextExtractor` — plain `.txt`, with optional `created:`/`aliases:` front-matter-style lines; a `# Heading` or an ALL-CAPS first line becomes the title. |
| `src/formats/markdown.rs` | `MarkdownExtractor` — `.md`/`.markdown`; front matter `aliases:` and `tags:` as `a, b`, `[a, b]` or a YAML block list (tags normalized: lowercase, `#` stripped); front-matter scalars are unquoted; the page date comes from `updated`/`modified`/`date`, then the same keys under a one-level `metadata:` block, then a `YYYY-MM-DD` file-name prefix. |
| `src/formats/code.rs` | `CodeExtractor`, a thin wrapper turning `wiki_code::extract`'s `CodeFile` into an `Entity`. |
| `crates/wiki-code/src/formats/code.rs` | Shared extraction core: `extract_code`, `default_cut`, `Shape`, `Rank`, `ItemKind`. Holds the language-neutral core; grammar node kinds live behind the `Shape` hook in each language's module, except the parameter-list reading for argument counts (`arity_of`, in `code/calls.rs`). Its children are the next four rows. |
| `crates/wiki-code/src/formats/code/lang.rs` | Per-language specs: `LangSpec`, `Placement`, extension lookup and `source_rank`, and the process-wide tree-sitter query cache (`QUERIES`). |
| `crates/wiki-code/src/formats/code/calls.rs` | Call-site extraction: attaches each call to its enclosing function item, with import aliases, arities (`arity_of`) and the shell and macro special cases. |
| `crates/wiki-code/src/formats/code/signature.rs` | Signature rendering: `build_signature`, `render_span`, `tidy_punctuation`, `collapse_runs`. |
| `crates/wiki-code/src/formats/code/doc.rs` | Doc extraction: plain-comment doc blocks, one-line item docs and the leading doc of a file. |
| `crates/wiki-code/src/formats/extract_rust.rs` | The Rust `LangSpec` and `rust_shape`: bare-`pub` visibility gating, owner qualification through `impl`/`trait` scopes and through struct/union/enum bodies, `#[macro_export]` gating for `macro_rules!`, `#[cfg(test)]` module stripping, and the `enum_variant`/`macro_definition`/`const`/`static`/`type`/`mod` signature shapes. |
| `crates/wiki-code/src/formats/extract_python.rs` | The Python `LangSpec` and `python_shape`: class-chain owner qualification, `__all__` handling, module docstring extraction, and the `assignment` signature shape. |
| `crates/wiki-code/src/formats/extract_cpp.rs` | The C++ `LangSpec` and `cpp_shape`: owner qualification through class/namespace scopes via `::` (replacing, not prepending to, a qualifier an out-of-line definition already wrote), template-head retention, no visibility gate (a header's declared surface is extracted regardless of `public`/`private`), and declaration ↔ definition pairing (`pair_declarations`, in `extract_cpp/pairs.rs`: same stem via `source_rank`/`module_stem`, taking only body-less header declarations and guarded definitions with out-of-line `= default` as a definition, then project-unique keys guarded by `pair_role`). |
| `crates/wiki-code/src/formats/extract_simple.rs` | JS, TS, and Go — three specs with no owner resolution, gated by `export_statement` or (Go) a leading-capital naming convention. |
| `crates/wiki-code/src/formats/extract_jsx.rs` | JSX and TSX — the JS and TS specs reused unchanged, TSX on the grammar that accepts JSX in expressions; `index.jsx`/`index.tsx` are directory modules. |
| `crates/wiki-code/src/formats/extract_vue.rs` | Vue SFCs — top-level `<script>`/`<script setup>` blocks go to the JS/TS/JSX/TSX spec their `lang` names; `<template>`, `<style>` and custom blocks are blanked in place so every byte offset still points into the file. Holds the block scanner `extract_svelte` reuses. |
| `crates/wiki-code/src/formats/extract_svelte.rs` | Svelte components — the same blank-in-place view over the instance and `context="module"`/`module` scripts; every top-level element but `<style>` is nested markup (so a `<script>` in `<svelte:head>` is not the component's), void elements and `{...}` expressions are skipped. |
| `crates/wiki-code/src/formats/extract_shell.rs` | Shell (`.sh`, `.bash`, or no extension and a `sh`/`bash` shebang, `shell_shebang`): functions, listed as `name()`, and the first top-level `NAME=` of each UPPERCASE name as a nested `value` item that only `search` finds by exact name; doc from the `#` run above; `source`/`.` arguments as imports; a file the grammar rejects is reparsed from a blanked copy, then a line regex adds functions still missing; calls outside every function hang off a `top_level` item that only `callers` shows. |
| `crates/wiki-code/src/summary.rs` | `summarize()` — deterministic, no-LLM one-line summary via a fallback chain (front-matter desc → docstring → first sentence of body → first signature). The sentence is cut from the whole opening paragraph (consecutive non-empty lines joined by a space), so a sentence wrapped at 80 columns in a `//!` block or README prose is not truncated at the line break. A sentence ends only at `.`/`!`/`?` followed by whitespace or end of line (so `index.json` does not end one), and lines opening "This document/page/module/file/note" are skipped as boilerplate. A list item ends the paragraph, and a page that opens with a list is summarised by its first item. |
| `src/model.rs` | Core types: `Entity`, `Edges`, `Graph`, `LintReport`, `SourceKind`, `KindFilter` (the `code`/`docs`/exact-kind search filter), re-exported from `wiki_code`: `CodeItem` (with the `test` flag) and `is_test_path()` (`wiki_code::model`), `title_case()` — the one casing rule every name-deriving path shares — and `normalize_path()` (`wiki_code::support`); `slugify()` — folds a name to an id matching `[a-z0-9_]+` in a single pass (Unicode-lowercase; Cyrillic transliterated, Ukrainian KMU 2010 plus Russian letters; Latin-1/Extended-A diacritics folded to the base letter; apostrophes between Cyrillic letters and combining marks dropped; alphanumeric kept, any run of other characters collapsed to one `_`), falling back to an anonymous `page_<hash>` id if nothing alphanumeric survives the fold. |
| `src/calls.rs` | `resolve_calls()` — call edges between code items, matched by name and argument count after the graph is built; `Wiki::calls` — what `neighbors --symbol` prints; `Wiki::callers` — what `callers` prints. |
| `src/graph.rs` | `build_graph()` — mention edges (phrase index, filtered by `refers_to_module` for code targets), import edges (`wiki_code::imports::Imports`: stem and `defined`-name segments under local roots), markdown-link edges (`resolve_link`), and PageRank. |
| `src/hash.rs` | BLAKE3-based `hash_bytes`/`hash_str`/`combine`/`to_hex` used for content hashes and render fingerprints. |
| `src/rewrite.rs` | Page rendering (`render_page`), fingerprinting (`render_fingerprint`), `## Notes` section preservation, atomic file writes (`write_atomic`); code-block masking, byte-offset preserving: `mask_fenced_code` backs `parse_sections` (so a `##` heading quoted in a fence never becomes a section, for rendering and search alike), and `mask_code` adds `mask_inline_code` on top for lint's link scan. |
| `src/cache.rs` | `.wiki/cache.json` — versioned incremental-render cache (`Cache`, `load`, `save`). |
| `src/manifest.rs` | Builds `Manifest` and renders `index.json`, `items.json`, `index.md`, `llms.txt` (summaries in `index.md` and `llms.txt` clipped to `LINE_CHARS`; pages at or above the median PageRank under `## Docs`, the rest under `## Optional`), `AGENTS.md`, `graph.json`. |
| `src/lint.rs` | `lint()` — in-memory broken-link, orphan-page and too-large-page checks over rendered pages. |
| `src/query.rs` | `Wiki` — loads a compiled output dir (`index.json`, `items.json`) for `search()` (in `query/search.rs`; page hits with symbol lines; one Aho-Corasick automaton over the query's tokens, built once per search, counts every token in a page body in one pass) and `neighbors()` (context-pack) queries (both in `query/resolve.rs`), with `resolve_id()` mapping an id, title or path to a page (on a miss, pages of the same file name are suggested first), plus `page()`/`has_page()`/`list_pages()`; used by the CLI, the MCP server, and as a library API. Children: `query/dates.rs` (dates written in a query), `query/tokens.rs` (tokenizing, stemming, field-word matching), `query/score.rs` (scoring a page against a query), `query/search.rs`, `query/resolve.rs`. See [Query internals](#query-internals). |
| `src/outline.rs` | `Wiki::outline` / `Wiki::page_text` — what `wiki page` and the MCP `page` tool print: a header, imports and one line per `items.json` item (`start-end  kind  signature`), or a markdown page's headings. Under `max_tokens` it cuts test items, docs and imports in that order, then trailing items, and ends with a `… dropped: … — pass max_tokens: N for all` marker. `page_text` with `full` returns the page, or its outline / a whole-line prefix when over budget. See [Query internals](#query-internals). |
| `src/labels.rs` | `labels()` — a Markdown file's ATX headings and bold step lines with the line range each covers; `doc_search()`/`doc_lines()` — `search` with `docs` (CLI `--docs`): labels holding the query's words in a row, else (named on a first line) labels opened by its leading ID, else labels holding all its words but `STOP_WORDS` in any order, else the table rows (`rows()`) holding its words in a row (an empty answer is `DOC_NO_MATCH`), read from the named files at call time, 10 hits by default (`DOC_LIMIT`), an empty query listing the first 200 (`DOC_LIST_LIMIT`); a label repeated under one head keeps its first and last entries when the limit cuts it. |
| `src/serve.rs` | `wiki serve` — MCP server over stdio (newline-delimited JSON-RPC; calls that read the wiki run on `WORKERS` threads, `ping` and the rest on the reader; `notifications/cancelled` drops a queued call or ends its lock wait (`dirlock::CANCELLED`) and sends no reply; tool schemas in `serve/tools.json`, written by hand: no `$schema`, `null` types or `format`; `search`'s `tag` and `format`, `page`'s `full` and `callers`' `max_tokens` are parsed but not in the schema, which `tool_schemas_match_the_params_structs` lists by name): `search` (`format`: `lines`/`json`; `search_lines`, `search_json`, in `serve/render.rs`; `limit` 5 by default; `docs` goes to `labels::doc_search`)/`page`/`callers` tools, plus `neighbors` and `lint` when `--tools` names them (`TOOLS`, `DEFAULT_TOOLS`); no resources. Without `--source`, `WikiState` lazily reloads the compiled wiki when `index.json`'s fingerprint (mtime, len) changes; with it, `WikiState` takes snapshots from `Live`. A failed reload keeps the last good snapshot, and so does one that would wait past the call's 5 s (`WAIT`) for a compile's `compile.lock`, adding `recompile still running` (`lint` then lints the snapshot); stale snapshots add a `wiki may be stale: <reason>` line to tool results. |
| `src/live.rs` | `Live` — `serve --source`: watches the project, recompiles in a background thread until the compiled generation catches up with the event generation (the worker compiles once events have paused for 150 ms (`watch::DEBOUNCE`), at most 2 s after it first sees an edit, which is after any running compile ends), and holds the current `Wiki` snapshot plus the stale reason. Each tool call writes a barrier file in `<dir>/.serve/`, waits for its event, then waits for the compile (both within one 5 s bound); while the start-up compile of an existing wiki runs, a call waits at most 1 s for it and then answers from the old wiki. Several servers may share one `--dir`: only the one holding `.serve/writer.lock` compiles; the others are readers that wait for the writer's `ack-*` file and reload (bounded by the call's deadline: a compile still holding `compile.lock` then leaves the old snapshot with `recompile still running`), and take the lock over when the writer exits. Once its binary is replaced, a server stops compiling and gives the lock up (the check is in `live/exe.rs`; the barrier and ack protocol in `live/barrier.rs`). At start-up the one that becomes the writer prints one stderr line, `wiki: this wiki was compiled by <old>; compiling it again with <new>`, when the output on disk was made by another version. |
| `src/dirlock.rs` | Advisory locks under `<dir>/.serve/` (`SERVE_DIR`): `try_writer` for `writer.lock` (one writer per `--dir`), and `compile.lock`, held `exclusive` while a compile writes (`serve`, `wiki compile` and `wiki setup` alike) and `shared`/`try_shared`/`shared_until` (polls until a deadline, so a call's wait stays bounded) while a load reads, so a load never sees half a wiki. |
| `src/hint.rs` | `hint_lines()` — `wiki hint`: code-only search plus, when the prompt passes the shared memory gate (`Wiki::passes_gate`, the one `search`'s tail uses), `MEMORY_HEADER` and one `memory (<origin>): <path> — <summary>` line (`serve::memory_prefix`) for the most trusted passing note (`Origin::trust`; the best scored of equals), the code `HEADER` only when there are code lines; pages scoring at least half the best kept, symbol lines deduplicated by `path:start-end`, capped at `HINT_LINES` and `LINE_CHARS`; no code lines when the best page scores below `HINT_MIN_SHARE` (0.5) of the prompt's available score (`PassStats::available`, the summed idf of its counted words), when fewer than `HINT_MIN_TOKENS` of its counted words occur in the project (document frequency above 0), or when `rests_on_function_words` finds that the prompt without its `FUNCTION_WORDS` (Ukrainian and English) has too few tokens, no hit, or a best page under `HINT_MIN_SHARE` (a second search, run only when the hint would otherwise fire); these two gates only silence, a kept hint is unchanged; empty when nothing matches, on a harness-written turn (`is_harness_prompt`: text opening with a tag or one of Claude Code's own notes, `HARNESS_OPENINGS`) and on a prompt with fewer than `HINT_MIN_TOKENS` (2) words of two or more characters (`Wiki::prompt_tokens` drops one-character words from every prompt search). `prompt_from_hook_json()` reads Claude Code's `UserPromptSubmit` input. |
| `src/generator.rs` | `generate_corpus()` — deterministic synthetic-corpus generator (SplitMix64 PRNG) used by `wiki generate` and tests. |
| `src/setup/mod.rs` | `run()` — `wiki setup`: compiles `.` into `.nollm-wiki` (with the agents' memory folders it finds, each reported), creates `.agents/memory` in both scopes (in local scope also listed in `.git/info/exclude`) unless `--no-memory` (`SetupOptions::memory` off: no folder, no memory compiled, the server registered as `serve --source . --memory none` by `entry`), then per agent (Claude Code, pi, Cursor, Codex) writes the MCP entry (`.mcp.json`, `.pi/mcp.json`, `.cursor/mcp.json`, `claude mcp add-json … --scope local`, or, for Codex, a block in the project's `.codex/config.toml` (`codex.rs`)) in the chosen memory variant, either variant counting as its own; when this `wiki` is the nollm-wiki home's `bin/wiki` (`nollm_wiki_home`), every entry, the Claude Code hook and the pi extension start it through `LAUNCH` (`sh -c 'exec "${NOLLM_WIKI_HOME:-$HOME/.nollm-wiki-home}/bin/wiki" …'`; pi builds the same path itself), and an entry that starts it by name counts as its own too and is rewritten; otherwise they start `wiki` by name; Codex's navigation steps as a marked block in `AGENTS.md` (`navigation.md`), pi's shorter steps as the same marked block in `.pi/APPEND_SYSTEM.md` (`navigation-pi.md`, `pi_navigation`: skipped while pi's agent dir, `PI_CODING_AGENT_DIR` or `~/.pi/agent`, has an `APPEND_SYSTEM.md` and the project has none, and a warning when that dir's `AGENTS.md` holds a navigation block; the agent dir is only read), pi's `wiki-navigation` skill (`skill_text`: its Memory section, naming `.agents/memory`, only with memory) and the ignore lines; Claude Code gets no skill. With no agent (`SetupOptions::agents` empty: none detected and none named or chosen) it still compiles, writes the ignore lines and (unless `--no-memory`) the memory folder, and exits 0; `main.rs` says that no agent was set up (or, with `--remove`, that nothing agent-specific was removed); `--remove` undoes only what it wrote. The prompt hook (Claude Code's `UserPromptSubmit` entry, pi's `wiki-hint` extension) is written only with `SetupOptions::hook` (`--hook`); a run without it removes one an earlier setup wrote, matched as `--remove` matches it (the pi extension also in its pre-opt-in text, `pi_hook(cmd, true)`), skips a file git tracks in local scope, leaves a file it cannot read with a `left <file> as it is: …` line without failing the run, and prints one `removed …: the prompt hint is off by default now` line per hook removed. Reports one line per item and returns `false` when anything was skipped. `find_on_path()` looks binaries up on `PATH`; `detect()` finds the agents of a project, and `parse_agents()` reads the answer when none is found and setup asks. |
| `src/setup/edit.rs` | Owned edits: JSON entries, files, marked text blocks and ignore lines recognised by exact value or content, so they can be added, left alone when foreign, and removed. |
| `src/setup/codex.rs` | `run()` — Codex's server in the project's own `.codex/config.toml` (no `codex` CLI needed for it): reads the file through the guards (inside the project, no link, one name, UTF-8, not Codex's own home: `CODEX_HOME`, else `~/.codex`), writes, switches or removes the block through `Report::file_item`, then looks for a machine-wide `wiki` server: on every run that may edit the file it asks `codex mcp get wiki` from the temp dir (so a trusted parent project's entry is not seen) and, once the block is in place (or, with `--remove`, gone), reports one it finds as another server (never changed): setup on the run that found the file without a `wiki` of its own (exit 1), `--remove` as a note on every run. `codex_home()` is also used by `detect()`, so a `.codex/` that is Codex's home is no project. |
| `src/setup/codex/toml.rs` | The text side of the block, with no TOML library: `state()` calls the block its own only when the header `[mcp_servers.wiki]` is followed by exactly the `command` and `args` lines of one of the entries `wiki` itself writes (`Ours` for this run's, else `OursOther`); anything else that may define the server (extra keys, quoted or dotted spellings, an inline `mcp_servers`, a byte order mark) is `Other` and left and reported, an `mcp_servers` it cannot join is refused too, so a second definition is never appended. `with_block`, `with_body` and `without_block` keep every other line byte for byte, and the line ending of the file. |
| `src/setup/SKILL.md` | The skill text, embedded with `include_str!`. |
| `src/setup/navigation.md` | The navigation steps `setup` writes into `AGENTS.md` for Codex. |
| `src/setup/navigation-pi.md` | The shorter navigation steps `setup` writes into `.pi/APPEND_SYSTEM.md` for pi, with the `mcp__wiki__*` tool names of the server it writes into `.pi/mcp.json`. |
| `src/watch.rs` | What `live.rs` watches with: `start()` — the watch setup, with an event hook; `is_change()` — rejects reads (`Access` events: inotify reports every open, the compile's own walk included); `WatchSet` — judges a path by the compile's ignore rules; `DEBOUNCE` — the 150 ms quiet window. |
| `src/main.rs` | CLI entry point (`clap`): `compile`, `neighbors`, `callers`, `page` (`--full`, `--max-tokens`), `search` (`--kind`, `--tag`, `--docs`, `--format lines|json`), `lint`, `serve`, `hint`, `setup`, `generate`. |

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

`disambiguate_ids` (`src/ids.rs`) gives every entity a unique id. An
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
  and the id tie-break (in the `query/search.rs` and `query/resolve.rs` sort keys for search-hit ranking) means
  equal scores still resolve to one fixed order instead of falling back to
  undefined behavior.
- **The synthetic corpus generator uses a hand-rolled, explicitly-seeded
  SplitMix64 PRNG** (`generator::generate_corpus`), not `rand`'s
  OS/thread-seeded RNG — the same `--seed` must produce the same corpus on
  any platform or Rust version, which an OS-entropy-seeded generator cannot
  guarantee.
- **Path containment (`is_under`, in `wiki_code::support`) compares lexically
  absolute paths** (`std::path::absolute`, no filesystem access or symlink
  resolution) rather than canonicalizing — canonicalization depends on the
  filesystem's current state (symlinks, mounts), which varies by machine;
  the lexical form doesn't.
- **Grammar shape is part of the output contract.** Extraction depends on
  tree-sitter field names (`trait:`, `type:`, `value:`, `body:`) and node kinds
  (`declaration_list`, `const_item`, `static_item`, `type_item`,
  `function_signature_item`). `code/Cargo.toml` pins
  `tree-sitter-rust = "=0.24.2"` (tree-sitter and every grammar with `=`), so
  that, not `Cargo.lock`, is what guarantees byte-identical output across
  machines; a grammar bump is an output-affecting change and must
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
  (`src/query.rs`, `tests/query.rs`), `wiki_code::imports::Imports` keeps the smaller
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
reads its front matter (or, for a page compiled by 0.6.7 or earlier, its `## Metadata`
bullets), and maps the `source` back to **every**
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

**Id resolution** (`Wiki::resolve_id`, used by `neighbors`, `callers` and `page` in
the CLI and MCP) tries, in order, and takes the first that names exactly one page:
the exact id; the title (case-insensitive); the slug of the argument; the
argument normalised (`\` to `/`, one leading `./` stripped) as an exact page
path; a unique page path ending in `/<argument>` (several are an error listing
them); for an argument longer than a page path, such as an absolute path, the
longest page path such that the argument ends with `/<root>/<path>`, where
`<root>` is `index.json`'s `root` (the compiled directory's own name), so a
path into another project with the same layout is not taken for this one's;
a wiki without `root` skips this step. Otherwise the error lists up to five
ids sharing a word with the argument.

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
  longer hits `process-wide`, and `wikilink` still hits `wikilinks`. A
  field word (item name words and docs included) that only shares the
  token's Snowball stem (`stem`: Ukrainian for a Cyrillic part, English
  otherwise) hits at `STEM_TIER` (0.3) of the field's weight, so
  `provisioning` finds `provision` below an exact or prefix hit. The
  body hits by substring, never by stem. Item names (`items.json`, lowercased, or one of
  their snake/CamelCase words) hit by whole term only, so `to` does not hit
  `token_estimate`. Two adjacent name words of at most 2 characters each
  also hit joined (`Wiki::match_words`: `QoS` is `qo`, `s` and `qos`); the
  join is a match word only, and the spelled-name check (`W_SYMBOL` for a
  query of exactly an item's words) and `name_sets` keep `name_words`.
  Joining when either word is short (`ofin`, `byid`) changed 16–44% of a
  tree's names and cost 2 ranks on a 1,350-query sweep; the narrow rule
  changed none there. Fuller
  matches are not forced above partial ones by a sort key; IDF does that
  where it matters, by making a rare missing token expensive and a common
  one nearly free.
- Score is BM25-shaped, computed in two passes over the pages that pass
  the `kind` filter. Per token: `idf = ln(1 + (N − df + 0.5)/(df + 0.5))`
  over those pages; body term frequency saturates as
  `tf·(k1+1)/(tf + k1·(1 − b + b·len/avglen))` with `k1 = 1.2`,
  `b = 0.75`; the token's contribution is `idf × (name 3.0 + alias 2.0 +
  summary 1.5 + heading 1.0 + body 1.0 × tf')`. Code items add three more
  page fields: the whole item name, also as `owner::name` (`W_SYMBOL` 2.8,
  below the title weight), else a word of an item name, method or not
  (`W_SYMBOL_WORD` 2.0), and the words of item doc sentences
  (`W_SYMBOL_DOC` 0.75); a token the first sentence misses but the rest of the
  item's doc (`CodeItem.doc_text`, never shown) has scores half that
  (`W_SYMBOL_DOC_REST`; on item lines `IW_DOC_REST`). Test items (`CodeItem.test`) and module items
  feed none of them, and a word that is already one of the page's own
  title words is skipped; other pages' title words stay. The `defined` and
  `methods` lists in `index.json` do not score: every name on them is an
  item. When
  the query's words are exactly the title's words, in any order, every
  token adds `W_TITLE` 3.0, so a title query finds its page even where
  other pages' item names share the title's words. When they are exactly
  the words of one item's name (`get bin stock` for `getBinStock`), every
  token the page title does not already match adds `W_SYMBOL`.
  When they all sit inside one longer item name (`order reject` in
  `onOrderReject`), each such token adds `W_IN_ONE_NAME` 1.0 instead. A token
  that is the page's file name with its extension or a path suffix of it,
  from a folder boundary, in any case (`names_file`), adds `W_FILE` 6.0 times the
  idf of the N pages it names (not its `df`, which counts every includer), over
  `sqrt(N)`; not on a memory note and not for a prompt (`search_prompt`). Field weights are not length-normalised, and `tf'` stays below the name weight, so a title
  hit beats any volume of body text on the same token. Because `N`,
  `df`, and `avglen` come from the filtered set, `score` is a ranking
  key, not a stable property of a page.
- A date written in the query (`query_dates` on the raw text: ISO, `D.MM[.YYYY]`,
  `<day> <month name>`, `<month name> <day>`) acts as one more token: a memory
  note whose `updated` is that day (any year when none is given) scores
  `W_TITLE` for it, with its own `df`/IDF; the words of the date still match
  as usual. `wiki hint` lets such a hit through its gate (`dated_on`).
- Hits sort descending by score (`total_cmp`), then by PageRank descending,
  then ascending by id. PageRank is computed over project pages only: a
  note's links still render but never move a code page.
- `search_tagged` with a query ranks the code and docs as if no notes
  existed (`MemoryScope::Without`: `search_terms` drops notes before `N`,
  `df` and `avglen` are counted), then `memory_tail` appends up to
  `MEMORY_TAIL` (2) live notes from `MemoryScope::Only`, most trusted first
  (`memory::Origin::trust`), then best score (newer `updated` first on a tie), after
  the hits and outside `limit`, whatever `kind`; with `tag` set a note must carry it. A note enters the tail
  through `passes_gate`, which `wiki hint` applies with two changes for a
  prompt (the name branch only up to `HINT_SHORT_WORDS` distinct words, the
  ratio branch of a longer prompt only against a code hit, and its note
  scored by `search_prompt` on the same `HINT_TERMS` words as its code): it
  is not superseded, and
  a query word of `gate_len()` (4) or more characters is in its title, aliases
  or summary (`names_page`), or the query writes its `updated` day
  (`dated_on`), or its score is at least `MEMORY_GATE_RATIO` (2) times the
  best code hit's. Note scores come from `MemoryScope::Only`, whose `N`, `df`
  and `avglen` count project pages and notes alike, so the ratio compares
  scores on the project's scale; only the candidates are notes. The
  environment variable `WIKI_MEMORY_GATE` sets the gate length; it exists for
  measurement and is not a user setting. `lines` prints a tail row as
  `memory (<origin>): <path> — <summary>` (the title when the summary is empty) and, when the hit has one, its snippet on an indented line; `page` on a note returns its text, JSON
  as a full hit; tag-only listings have no tail and mark a superseded note.
- The searched body text is the rendered page's parsed sections minus
  generated chrome (`Metadata` of pages from <= 0.6.7, `Related`, `Imported by`, `Mentioned in`,
  `Notes`) —
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
  (`IW_WHOLE`, also for every token when the query is exactly the name's
  words), a name word 2.0, a doc word 1.0; test items are scaled by 0.5
  (`TEST_ITEM_FACTOR`). A whole-name match outranks any word match even
  with a doc match (a compile-time assert pins it). Per page the top 3
  (`SYMBOLS_PER_PAGE`) distinct items are kept, ties by line; across the
  response a line must score at least 0.3 (`MIN_REL_SYMBOL`) of the best
  line, except a page's own best line(s), which always show: a page with
  any matching item keeps its best one. Page score and order are not
  changed by expansion. A `SymbolLine`
  carries `path`, the 1-based inclusive line range, `owner::name`, the
  signature, and the doc, which is dropped when an earlier line of the
  same page shows the same one. C++ gives each declarator of a shared
  declaration its own signature, which therefore may not be verbatim in
  the file.
- **Named definitions** (`lift_names`) run after expansion, in `search`
  (`search_tagged`), not in `hint`. A query word with `_` or `::`, or an
  upper-case letter after the first, is an identifier (`identifier`: outer
  punctuation trimmed, `_` kept). Its definitions are the items named so,
  or `owner::name` so (`Inner::f` also names `Outer::Inner::f`),
  case-sensitively only (a name in another case is another item); modules and
  top-level items never; at most 3 (`DEFINITIONS_PER_NAME`) and `limit`,
  pages ranked higher first, then by pagerank, one per page before any
  page's second. A name with more than 3 marks its last shown line
  (`SymbolLine::hidden`), which the line output follows with
  `+N more named <name> on M other pages; narrow with Class::<name> or a
  path word` (`Class::` only when a hidden one has an owner; `--format
  json` has no marker). In a query of identifiers
  alone a name equal, in any case, to a page's `module_stem` puts that page
  (`stem_pages`, same cap and order) before its definitions, as a page with
  no lines: a module is no item. Such a query with any defined or stem page
  gets just those lines, page by page in the order names first land, with no
  per-page cap; `not_found` lists the names with neither and
  the line output prints `no symbol named <name>; the pages below only mention it` before the hits. There each
  non-nested function or method line carries `called_by`
  (`Wiki::caller_count`: the distinct callers of the item and its twin, as
  `callers` lists them one level up, `None` at 0), which the line output
  prints as `` [called by N: `callers`] `` after the signature. In any other
  query a missing definition line is appended to its page, or its page to
  the hits, and nothing moves: lifting named lines first cost three top-1
  answers on the search regression set, where the name was context. The
  memory tail's ratio gate uses the ranked best score, taken before.
- **The test flag.** `CodeItem.test` is set at extraction (`formats/code.rs`):
  the file is a test path (`model::is_test_path`: a `tests` path component,
  or a file named `test_*`, `*_test.*`, `*_tests.*`), or, for Rust, the item
  sits in a `#[cfg(test)]` module or carries `#[test]` (also
  `#[tokio::test]`). It is stored in `items.json`. Test items stay visible
  as symbol lines but contribute no page fields, so their names cannot
  lift their page; body text still counts.
- **Kind filter.** `KindFilter` accepts `code` (any `code:<lang>`), `tests`
  (code pages on an `is_test_path` path), `docs` (`markdown` and `text`) or
  an exact kind; the MCP `kind` and CLI `--kind` share it and its error
  text. In `search`, `code` and `code:<lang>` also leave out test pages
  (`KindFilter::drops_tests`, `Entry::shown`), and only there: `neighbors`
  with `code` and the prompt hint's internal `Code` filter keep them.
  `search_terms` counts `N`, `df` and `avglen` over every code page for both
  `code` and `tests`, scores, sorts, and only then drops the other side, so a
  page's score is the one it had before the split; filtering before the
  statistics moved 31 of 1,350 ranks on a generated sweep and none after.
  When a test page was in the unsplit top `limit` of a `code` search,
  `PassStats::tests_cut` is set, `Wiki::search_answer` returns it, and the
  line output ends `tests also match; use kind "tests"`. Printed whenever any
  test page matched, that line would fire on nearly every query. For a
  query of names alone, `Wiki::in_tests_only` picks the `not_found` names a
  `tests` search would find (a definition on, or the module stem of, a test
  page), and the line output gives each `<name>: in test pages only; use
  kind "tests"` in place of `no symbol named`.
- **Output.** `serve::search_lines` renders the default line format (page
  line `id  path  [date  ]title — summary`, indented symbol lines
  `path:start-end  name  signature  — doc`, or the snippet for a page
  without symbols; `no matches` when nothing hit); `serve::search_json`
  renders the JSON form. The MCP `format` parameter and the CLI
  `--format` choose between them; `lines` is the default for both.

**Neighbors** (`Wiki::neighbors`) takes a page id; both the CLI and the
MCP tool first pass what the caller typed through `Wiki::resolve_id`, which
accepts an exact id, a title (ignoring case), the slug of the argument, or
the one page whose source path equals it or ends with `/<arg>`. A failure
lists up to 5 closest ids (the pages an ambiguous path matched, else ids
or titles sharing a word, non-test pages first). When the ids came from the
shared word or from nothing, the error also gives up to 3 `defined in:` places
of items named like the argument's file stem (`Wiki::defined_as_stem`,
non-test files first). On a failure `Wiki::resolve` first asks
`Wiki::skipped_path`: a relative path whose outermost pruned directory
(`walk::pruned_dir`, the `SKIPPED_DIRS` and hidden-folder rule) is not at or
above the compile input's name (`root`), and under which no indexed page
lives, gets a note that the folder is not compiled. `neighbors` then BFS-collects ids to
`depth` hops over both edge directions, then builds a budgeted context pack:

- `--max-tokens` is a hard ceiling on the pack's estimated size, using
  the same chars/4 rule as `manifest::token_estimate`; every block is
  charged at the size of the text actually emitted, so the concatenated
  pack can't overshoot the ceiling.
- The target page comes first. A code page is its outline, cut to the
  budget; another page is in full when it fits, otherwise degraded to a
  title + summary block pointing at `page` with `full: true`. The outline's
  header and `…` line, or the degraded block, are the one floor
  exception: always emitted, even when a pathologically small budget
  cannot contain them.
- Neighbor admission walks the target's import neighbors (`imports_out`
  and `imports_in` in `index.json`, either direction) first, then the
  rest, each group in descending centrality (skip-not-break): a neighbor
  that doesn't fit the remaining budget is skipped and the walk
  continues, so the kept set favors high-centrality neighbors over
  maximum cardinality. An import neighbor is its whole outline when that
  fits, else a summary block like every other neighbor. `--max-nodes`
  truncates the candidate list before admission; `--kind` (the values of
  `search --kind`) drops neighbors of other kinds before it.
- Emission order is ascending PageRank — the highest-centrality neighbor
  lands last, closest to the end of the pack ("lost in the middle"
  placement). `--full` swaps neighbor summary blocks for whole outlines
  (code) and full page bodies (other pages).

**Calls** (`Wiki::calls`, `neighbors --symbol`) walks `items.json`'s
`calls` breadth-first from every function on the page named `symbol`
(a name or `Owner::name`; a paired declaration's definition is a root
too), to `depth` (no limit by default), and prints one
`start-end  kind  Owner::name` line per function reached, grouped by file
in the order the walk first reaches each file. Over `max_tokens` the
farthest functions are cut, and a last
`… N more functions — pass max_tokens: T for all` line says so.
**Callers** (`Wiki::callers`, `callers`) is the same walk over the
`calls` edges turned around (`callers_index`, built in `Wiki::load`), one
level by default, with test code last; each caller line ends with the
`call_lines` of the call that reached it.

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
excluded node kind (except a function directly in a function body, see
below), where Rust's `rust_placement` is an **allow-list** of
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
and `wiki_code::imports::Imports` uses that rank so a bare `#include "orderbook.h"` or a
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
ends the paragraph, in every language.** `leading_doc` used to stop
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
punctuation a wrapped parameter list leaves behind, for every language.
A multi-line Python `def` or Go `func` renders on one line with `( ` and `, )`
cleaned up. This is deliberate; the pass and its limits are documented on
`tidy_punctuation`.

**Rust test-module stripping:** before body assembly,
`strip_rust_test_modules` (`crates/wiki-code/src/formats/extract_rust.rs`) removes each
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
function-local definitions are not items, with three exceptions, each an
item with `nested` set, never public and found by `search` only: a Python
`def` directly in a function body (`Placement::Nested`, owner-less; one in a
class defined there stays out), a shell file's first top-level `NAME=` of
each UPPERCASE name, and a C/C++ class, struct or union data member (a
`value` owned by its class, `static constexpr` ones included). A nested
value matches its exact name only: word scoring and the any-case fallback
skip it. Outlines (`Keep::items`), the hint, `also defined`
(`same_name`) and the call graph skip it: it is no call target and owns no
call sites (`attach_call_sites`), so calls in its body stay its enclosing
function's. As targets, same-named helpers nested in different test
functions cross-wired (a C++ trading system, 2026-11-11: 68 edges lost, 145
wrong); without, the graph equals the one with no nested items. An item's range climbs from its
signature node over the contiguous leads directly above it
(`LangSpec::item_lead`: Rust attributes and `///` lines, Doxygen comments,
JSDoc blocks, Go `//` lines, with Go directives such as `//go:generate`
kept in the range but out of the doc), stopping at a blank line, at a lead
that trails code on its own line (`int n; ///< Count.`), or at anything
else, so a stray doc comment never stretches it. A C++ member-trailing doc
(`///<`, `//!<`, `/**<`, `/*!<`) stops the climb even on a line of its own:
it documents the member before it.
In C++, JS/TS (JSX/TSX and SFC scripts included) and Python, a plain
comment block (`//` lines, a `/* … */` block, `#` lines) directly above an
item is its doc when nothing better exists: a Doxygen or JSDoc doc comes
first, then a Python docstring, then the plain block (`plain_doc`). A plain
block above a doc comment neither joins its doc nor extends its range. The
block is no doc, and the range keeps its own start, when it begins on the
file's first line or names `SPDX-License-Identifier`/`Copyright`, when it
is only dividers and framed titles (`--- Risk ---`) or one title line
between dividers (a banner), all TODO/FIXME/XXX/HACK
lines, mostly code (half its lines end in `;`, `{` or `}`), or a group
label: one line of at most three words, no closing `.`, `!`, `?` or `:`,
sharing no word with the item's name ("Getters", but "Risk limits" above
`risk_limit` is a doc). Rust and Go plain comments still stop the climb.
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
  the source file they test when small (e.g. `src/cache.rs`, `src/hash.rs`),
  and in `<module>/tests.rs` next to the module when large (e.g.
  `src/query/tests.rs`, `src/ids/tests.rs`; `src/tests.rs` for `src/lib.rs`)
  — colocated with the code, not in a separate tree.
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
  - `tests/hint.rs` — `wiki hint` end to end: code lines for a task text,
    silence when there is nothing to say.
  - `tests/labels.rs` — Markdown headings and bold step lines with their
    line ranges, behind `search --docs`.
  - `tests/page.rs` — `Wiki::outline`: a page's module map against a
    compiled fixture.
  - `tests/query_defined.rs` — item names in search: an older `index.json`
    without `defined`, and the whole-term match rule.
  - `tests/setup.rs` — `wiki setup` end to end, with stub agent binaries.
  - `tests/shell_extraction.rs` — shell scripts through `compile()`:
    functions, docs, the parse-error fallback, page ids.
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
  - `tests/query_list_pages.rs` — `Wiki::list_pages` id/title ordering.
  - `tests/mcp_serve.rs` — end-to-end MCP: spawns `wiki serve` and speaks
    raw newline-delimited JSON-RPC over stdio, covering the tools.
  - `tests/eval.rs` — the retrieval eval harness. Compiles the frozen
    17-page corpus in `tests/.eval_corpus/` and scores 22 labelled queries
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
    exactly (e.g. `21.0 / 22.0`); never transcribe them from the `{:.4}`
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
  and `symbol_hit@3` floors per split (`file_hit@3` tune 35/35, held 17/17;
  `symbol_hit@3` tune 34/35, held 17/17) and the
  title-as-query count (28/28). The `neighbors` pack size is recorded
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
  `.github/workflows/rust.yml` runs the same on every push to `main` and
  every pull request, in five steps: `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings`, `cargo test`, `scripts/test-strip-chrome.sh`,
  `cargo doc --no-deps` with `RUSTDOCFLAGS=-D warnings`. Clippy's
  `pedantic` group is on through `[lints.clippy]` in `Cargo.toml`, with the
  lints that fight this codebase allowed there, each with its reason; `-D
  warnings` turns the rest into failures. Two more workflows: `audit.yml`
  checks `Cargo.lock` against the RustSec database on every lockfile change
  and weekly, and `release.yml` builds Linux and macOS binaries for a `v*`
  tag and attaches them, with their `SHA256SUMS` and `manifest.json`, to a
  draft GitHub Release with generated notes, which the owner signs
  (`scripts/release-sign.sh`) before publishing it.
  `.github/dependabot.yml` opens weekly dependency PRs, minor and patch
  bumps grouped into one.
