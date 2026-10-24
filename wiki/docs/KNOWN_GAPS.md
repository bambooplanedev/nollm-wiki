# Known gaps

Places where the wiki is known to fall short: wrong or missing results,
behaviour that surprises, and what agents actually do with it. Deliberate
scope cuts (no embeddings, no LLM, three formats, query-only MCP) are in the
README under [Limitations & roadmap](../README.md#limitations--roadmap); the
compiler's internals are in [ARCHITECTURE.md](ARCHITECTURE.md).

Numbers below come from field runs on a 2,746-page C++/Rust/Python project
with a 12B model (66K context) in pi, unless a line says otherwise. Two runs
per question is noise-level: treat single results as anecdotes.

## Agents and the wiki

- **Small models skip or misread the skill.** pi lists the skill with its
  correct location, but the model often reads
  `~/.pi/agent/skills/<name>/SKILL.md` by analogy with the user's other
  skills, gets ENOENT, and falls back to `find`/`grep`. It also picks a
  competing skill (`systematic-debugging`) for a "where is" question.
  Anything the model must know only from the skill is at risk.
- **Whole-file reads after a hit.** `search` returns `path:start-end`, yet
  the model reads the whole file (about 51 KB per read, 3–6 reads a run)
  and re-derives end lines with `grep`. This, not the wiki call, is what
  runs out the 300 s cap and the context.
- **`neighbors` goes unused.** Zero calls in 27 unprompted runs, including
  questions that ask for a call path. `symbol` is named only in the skill
  and the tool description; see the previous point.
- **The hook trades tools for hints.** With `wiki setup --hook` the agent
  reads the injected lines and almost never calls a tool, so a wrong hint
  line is not corrected by a search.
- **pi does not show MCP server instructions** for `direct` servers (pi
  1.3.0); only tool names, descriptions and parameter docs reach the model.
- **pi loads project files only after trust**: `.pi/mcp.json` and project
  skills need `/trust`, or `--approve` in print mode.
- **Agents edit despite "do not edit".** Seen twice in field runs, once
  applied. Run evaluations with edit tools excluded
  (`pi --exclude-tools edit,write`) and check `git status` after.

## Search

- **Vocabulary is literal.** No synonyms: "credentials" does not find
  `creds`. Cyrillic query words are stemmed by a fixed ending list, on the
  query side only, so a stem can also hit an unrelated word that shares it.
  English and other Latin-script words are not stemmed: "decision" does not
  find `decide`. Doc text helps only when the comment uses the user's word.
- **Docs crowd out code without `kind`.** In a project with many spec and
  plan pages, 2–3 of the top hits for a code question are docs or tests
  unless the query passes `kind: code`.
- **Vendored code ranks like project code.** `third_party/` hits (e.g.
  `TooManyInputsFlag` for the word "too") take result and hint slots.
- **A decision split across a function and an enum's docs is not found.**
  "Where does it refuse to start" lands on the file, not on `decide`.
- **`wiki hint` shows at most 2 lines per page**, so a third relevant
  symbol in one file is not shown.

## Call chains (`neighbors` with `symbol`)

- **Resolution is by name, not by type.** A member call on a variable
  resolves to the methods of that name the caller's files see, else to the
  one project method with that name. So `.size()` on a `std::vector` lists
  `TaskRing::size`, `e.what()` can reach a vendored `exception::what`, and
  a call through a base pointer lists whichever of base and overrides are
  visible.
- **All C++ overloads of a name are listed**; arguments are not matched.
- **Not seen:** callers (there is no reverse walk), Python module aliases
  (`import x as S; S.f()`), functions passed as values (callbacks, function
  pointers, lambdas stored and called later), and calls inside Rust macro
  arguments (`format!("{}", f())`), which tree-sitter keeps as tokens.
- **No calls across languages.** A Python binding calling C++, or FFI, is
  not an edge, by design.
- **Unresolved names are dropped silently.** Builtins and std calls
  (`len`, `join`, `std::move`) are correct drops; a project function the
  rules miss looks the same.
- Exactly-one rate over call sites whose name some project function
  carries: C++ bare calls 85%, Rust 97%, Python 73%. JS/TS and Go are not
  measured.
- `items.json` grows about 22% (3.3 → 4.0 MB on the project above).

## Extraction

- **Items:** a private `macro_rules!` is not an item, nor is a function in
  a Rust `extern` block. A JS/TS method of an exported class, or an
  exported variable, reads `public: false`. An object-literal or
  anonymous-class method is a free `fn` with no owner. The names of one
  multi-declarator statement share its line range. JS/TS values inside a
  top-level block (`if {…}`, `for (let …)`) and `require` bindings are
  items. Go `var f = func…` is a `value`, while a JS arrow is an `fn`.
- **C++ access is ignored:** every C++ item is `public: true`;
  `private:` sections are not read.
- **C++ declaration ↔ definition pairing** works on same-stem files in one
  directory. Repeated basenames across directories do not pair, and a lone
  cross-directory `x.h`/`x.cpp` with a coincidental free-function key
  could.
- **tree-sitter-cpp misparses some `requires` clauses** (a grammar bug).

## Comment docs

- A file header after a shebang or `#pragma once` can be taken as the
  first item's doc.
- Python `##` comment blocks and `/*** x ***/` blocks with trailing stars
  are not handled specially.
- A short doc on a value can be rejected as a group label
  (`// update check frequency` above `const DAILY`).
- A function's doc at the very top of a file is treated as a file header.
- A Doxygen/JSDoc lead right above a plain comment block keeps the plain
  block out.
- A macro line between a comment and its item stops the climb (e.g.
  `V8_CLANG_NO_SANITIZE("address")`).
- The mostly-code rule (half the lines end in `;`, `{` or `}`) also
  rejects a 1–2 line prose comment that happens to end that way.

## `page` and outlines

- `page` with `full` and no `max_tokens` has no ceiling: a large docs page
  comes back whole (20K chars seen), which a small agent then truncates.
- Headings: a bare `#` gives a blank line, a closing `##` is kept, and
  headings indented 1–3 spaces are skipped.
- The outline header carries the page summary uncapped (a 5 KB summary
  was seen on one pathological doc).
- `page` on an entry without a body says "unknown page".

## Notes and ids

- **Transliterated ids can be long.** A Cyrillic title becomes a Latin id
  with no length cap yet.
- **A date comes only from front matter or a dated file name.** Never from
  git or mtime, so a note without either shows no `updated`.
- **`large` measures the compiled page** against the default 4000-token
  `neighbors` budget, not the source file's size.

## `serve --source` and `--watch`

- Files written only through `mmap` are not seen as changed.
- `compile --watch` ignores an inotify queue overflow instead of rescanning.
- Barrier files left by a crashed server stay in `.wiki/.serve/`.
- `lint` and the `wiki://index` and `wiki://llms.txt` resources read disk,
  not the live snapshot.
- A generated file ignored at file level still triggers a recompile when
  it sits in a watched directory.
- inotify watches `target/` and `.git` too: check the project's directory
  count against `fs.inotify.max_user_watches`.

## `wiki setup`

- Agents: Claude Code and pi only.
- Local scope skips (exit 1) a file git already tracks.
- `--remove` leaves `.wiki/` and the ignore lines in place.
