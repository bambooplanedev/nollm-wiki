# Known gaps

Places where the wiki is known to fall short: wrong or missing results,
behaviour that surprises, and what agents actually do with it. Deliberate
scope cuts (no embeddings, no LLM, three formats, query-only MCP) are in the
README under [Limitations](../../README.md#limitations); the
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
- **Whole-file reads after a hit (pi, 12B).** `search` returns `path:start-end`, yet
  the model reads the whole file (about 51 KB per read, 3–6 reads a run)
  and re-derives end lines with `grep`. This, not the wiki call, is what
  runs out the 300 s cap and the context. Haiku in Claude Code read the given
  range in 57 of 58 reads.
- **`neighbors` goes unused.** Zero calls in 27 unprompted runs, including
  questions that ask for a call path. `symbol` is named only in the skill,
  the server instructions and the tool description; see the previous point.
  Off by default since 0.6.7 (`--tools` brings it back).
- **The hook trades tools for hints.** With the prompt hook the agent
  reads the injected lines and almost never calls a tool, so a wrong hint
  line is not corrected by a search. Measured 2026-10-28 with
  qwen3.8-27b and deepseek-v4.1-flash through OpenRouter:
  - On question tasks over three fixture repos it did not lower
    correctness and cut lines read.
  - On edit tasks with look-alikes next to the target it did not
    significantly lower correctness either (PASS 55/60 with the hook,
    60/60 without; p 1.0 and 0.11 per model). The hint's first line was a
    look-alike in 5 of 6 tasks, and the agent read that file too: one
    extra file read where the look-alike sits in another file.
  - The agent reads every candidate file of the hint in its first turn.
    Neither a doc on each hint line nor a header that says "read the one
    that fits" stopped it from reading a same-named twin.
- **The hook cost grew with the prompt, and few subagents use the wiki.**
  On the bench project (3,304 pages) `wiki hint` took about 5 ms per
  prompt word, 4.7 s on a 1,246-word subagent hand-back, because it
  scanned every page body once per word; it now takes 0.16-0.45 s flat,
  and harness turns and one-word replies are silent. On that project 5 of
  28 briefed subagents called the wiki: briefs already carry a median of
  22 file paths, instructions in CLAUDE.md, the brief or memory did not
  change tool choice, and the hook reaches only the main session, which
  delegates everything.
- **Claude Code agents grep more without the navigation skill.** Over 24
  Haiku runs on the project above, tools loaded up front and no skill gave 48
  Grep/Glob calls against 20 for the skill setup with deferred tools, yet a
  median 5 turns against 7.5 and 138K input tokens against 174K, so setup no
  longer installs that skill for Claude Code.
- **pi does not show MCP server instructions** for `direct` servers (pi
  1.3.0); only tool names, descriptions and parameter docs reach the model.
  Without the `wiki-navigation` skill, a user-installed extension that makes
  pi load a skill first (skill-first) led it in our runs to an unrelated
  skill (`systematic-debugging`). Numbers: README, pi section.
- **Two wiki servers in pi** (a global `~/.pi/agent/mcp.json` entry and the
  project's `.pi/mcp.json`) give the model every tool twice, under two
  names; the skill `wiki setup` writes names only `mcp__wiki__*`.
- **pi loads project files only after trust**: `.pi/mcp.json` and project
  skills need `/trust`, or `--approve` in print mode.
- **Agents edit despite "do not edit".** Seen twice in field runs, once
  applied. Run evaluations with edit tools excluded
  (`pi --exclude-tools edit,write`) and check `git status` after.

## Search

- **A named file is a weight, not a guarantee.** A query word that is a file
  name or a path suffix adds `W_FILE`, so a rare identifier on another page can
  still lead (the named file was first in 778 of 866 generated queries, top 5 in
  all), and a name on many pages (`mod.rs`) weighs little. `src/a.rs:120` and an
  absolute path do not count, nor does a file name with no extension. A hint
  ignores the rule: a prompt naming a file does not find it for that reason.
- **Vocabulary is literal.** No synonyms: "credentials" does not find
  `creds`. Query and field words are compared by their Snowball stem
  (Ukrainian for Cyrillic, English otherwise), so a stem can also hit an
  unrelated word that shares it, and two forms with different stems still
  miss: "decision" does not find `decide`. Body text is matched by
  substring, not by stem. Doc text helps only when the comment uses the user's word.
- **Docs crowd out code without `kind`.** In a project with many spec and
  plan pages, 2–3 of the top hits for a code question are docs or tests
  unless the query passes `kind: code`.
- **Doc labels match words in a row.** `search` with `docs` finds a label
  only when it holds the query words consecutively: `Step 5c flag above the
  stamp` misses `Step 5c: a flag above the stamp's …` over the `a`, and a
  paraphrase misses too. The empty answer says so and suggests fewer words
  or grep. An in-order fallback with gaps was measured and dropped: 0 gains
  on 380 lead grep terms, 1 wrong label, and 12.9% of empty two-common-word
  queries turned non-empty.
- **Process folders and vendored code are skipped by name.** A `superpowers`
  folder and `third_party`/`vendor`/`node_modules`-style folders are not
  compiled (on one bench project 2,358 of 3,298 pages, 71%, and 77% of the
  wiki's bytes). A project's own code in a folder of such a name is left out
  too, unless compiled with `--no-ignore`; with it, vendored hits (e.g.
  `TooManyInputsFlag` for the word "too") take result and hint slots again.
- **A decision split across a function and an enum's docs is not found.**
  "Where does it refuse to start" lands on the file, not on `decide`.
- **`wiki hint` shows at most 2 lines per page**, so a third relevant
  symbol in one file is not shown.
- **`wiki hint` is silent only on text whose words scatter.** Below half
  the prompt's available score it prints no code lines (2026-11-01: 80 of
  80 bench task prompts and code questions kept; 162 of 251 bench subagent
  briefs whose hint named no file the agent opened silenced, 26 of 75 that
  named one kept). Short chat with no task ("This is a no-op check. Just
  reply ack", a reviewer's severity/priority boilerplate) still gets lines:
  on 81 such texts over five wikis the best page scores a median 1.25 of
  the available score, above the code questions' lowest (0.59), and no
  measured statistic told them apart. Long task prompts lose lines too: 12
  of 35 that showed the page their task names went silent.

## Call chains (`neighbors` with `symbol`, `callers`)

- **Resolution is by name, not by type.** A member call on a variable
  resolves to the methods of that name the caller's files see, else to the
  one project method with that name. So `.size()` on a `std::vector` lists
  `TaskRing::size`, `e.what()` can reach a vendored `exception::what`, and
  a call through a base pointer lists whichever of base and overrides are
  visible.
- **Argument counts rule out only what cannot fit.** A Rust, Go or Python
  function whose parameters cannot take a call's arguments is no match
  (`.map(|x| …)` never reaches a two-parameter `map`). A C++ function is
  ruled out only by too many arguments, since a default can sit on its
  other declaration; JS/TS functions and decorated Python functions are
  never ruled out. So `.get(k)` on a `HashMap` still lists a project
  `Mapping::get(key)`, and all C++ overloads that can take the count are
  listed.
- **Not seen:** calls outside any function (module level, such as
  Python under `if __name__ == "__main__":` or a TS top-level `init();`;
  shell files are the exception, with a `<top level>` caller), functions
  passed as values (callbacks, function pointers, lambdas stored and called
  later), CommonJS renames (`const m =
  require('./x')`, `const { a: b } = require(…)`), a TS default import
  used as a namespace (`import d from './m'; d.f()`), and calls inside a
  `macro_rules!` body. An empty `callers` result says so and points to grep.
- **Rust macro arguments are read as tokens.** Any `name(…)` token tree
  inside a macro invocation (`format!`, `select!`, `assert_eq!`, …) counts
  as a call, so a tuple-struct or enum pattern in `matches!(x, Foo(y))`
  links a project function named `Foo` that takes that many arguments. A
  closure (`|a, b|`) or generic arguments at the top level of the call
  leave its argument count unknown.
- **Import aliases are one map per file and resolve by the last segment
  only:** `import * as m from './a/b'`, `import a.b as m` and Go `m "a/b"`
  make `m.f()` read as `b.f()`, so a second module named `b` elsewhere
  matches too, and two imports aliased `m` in different functions both read
  as the later one. A parameter or receiver named like the alias keeps its
  own name.
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
  `private:` sections are not read. Only `## Exports` would change, and a
  static scan misreads `public:` from macros (97 nanobind members) and
  specifiers under `#if`.
- **C++ declaration ↔ definition pairing** crosses files and directories
  when the key `(owner, name, arity)` is unique among all header items and
  among all implementation items. In both steps (same-stem and
  project-wide) a header item with a body never pairs, and neither does a
  `static`, anonymous-namespace or class-body definition in a `.cpp`. Two
  definitions of one key never pair. Uniqueness can still
  mislead: namespaces are not compared (a unique-key free function in
  another named namespace can pair with an unrelated header
  declaration; none observed), and a definition in an unindexed file
  (`.c`, `.mm`, `.cu`) is invisible, so a same-key `.cc` function would
  pair instead (none observed). The owner is the last scope only
  (`MapBaseObj::iterator::operator++` → `iterator`), so enclosing classes
  are not compared either. Adding a file that defines a same-key function
  unpairs a distant pair, by design. Templates: `Ring<T>::size` out of line
  reaches its definition in walks, but the outline and `callers` asked on
  the header are imprecise; not fixed. A fix was measured and dropped:
  stripping `<T>` from the owner touched 27 items, rewrote `## Exports` on
  9 pages and broke `callers` asked with `Ring<T>::size`, while the corpora
  have no out-of-line template member in an implementation file. A
  template member defined out of line keeps the owner `Ring<T>`, so the
  outline lists it in its own group and a definition in the header gets a
  second call edge next to its declaration. A leading macro can put the return type into the owner
  (`handle api<Derived>`), and a free function with a template-id return
  type can get that type as owner (`UniquePtr<TEST_INT>`).
- **Shell (`.sh`, `.bash`) lists only function definitions.** A function
  passed as an argument (`trap f EXIT`, `sudo f`, `command f`) is not a call
  of `f`, and a call in a part the grammar could not parse has no caller (unless a
  fully parsed function encloses it inside that part).
  tree-sitter-bash 0.25 rejects some code bash takes: a case pattern with a
  glob and a quote (`*' '*)`), `[ ] {` in a `${v#pat}` pattern, `;` in a
  `${v:+word}` word. A file with such errors is parsed again from a copy
  with those bytes blanked, same length and lines, and that tree is used
  when it has fewer ERROR lines; other gaps (e.g. `${v:$((n-1))}`, `[` in
  an extglob inside `[[ ]]`) still leave ERROR nodes.
  Functions the tree still misses come from a line regex: no doc, and
  a span that ends at the next `}` in column 0. Shell pages are reached by
  path only, never by module stem or defined name. A same-named page in
  another directory still shortens ids by directory, as any two pages do.
  In a file with parse errors, the line regex also reads function-like
  text in a here-document as a function, and that text can become the page
  summary. The blanking pass reads `$((1<<n))` as a here-document start and
  skips the rest of that file: the recovery is lost, never a wrong edge.
- **tree-sitter-cpp misparses some `requires` clauses** (a grammar bug).
- **C and C++ `#ifdef`/`#ifndef` branches that each open or close braces**
  can make the parser treat one function as running to a much later line, so
  that item's line range is too wide and items after it can be missed.
  Measured on sqlite: in `prepare.c`, `sqlite3InitOne` is reported as lines
  212-1128; `build.c` shows the same around `#ifndef SQLITE_OMIT_REINDEX`.
  Functions generated by macros (sqlite `ext/jni/sqlite3-jni.c`, `JniDecl*`)
  are not seen. Measured coverage against universal-ctags: redis 99.2%,
  sqlite 96.7%.

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
- Headings: Setext headings (a line underlined with === or ---) and
  headings indented one to three spaces are not read.

## Notes and ids

- **Long titles give long ids.** An id over 200 bytes (a long H1, a
  transliterated Cyrillic title) is cut and ends in a 16-hex hash of the
  full slug, so it fits a file name but no longer reads as the whole title.
- **H1 titles are cleaned of Markdown links and images only.** Images and
  badges are dropped and `[text](url)` becomes `text` (code spans are kept);
  HTML tags and other emphasis marks stay in the title and the id.
  A `[text][ref]` form is a link only when the file defines `[ref]: ...`.
  Brackets nested in alt text and URLs containing parentheses are not parsed.
- **A date comes only from front matter or a dated file name, and only for
  markdown pages.** Never from git or mtime, so a note without either shows
  no `updated`.
- **Ids changed for Cyrillic and accented titles.** After upgrading the
  binary, recompile the wiki: linting an old wiki with a new binary (or the
  reverse) reports false broken links.
- **`large` measures the compiled page** against the default 4000-token
  `neighbors` budget, not the source file's size.
- **Body links to non-pages stay unresolved in Obsidian.** A `[[x]]` whose
  slug is a page, and a relative Markdown link to an indexed page, are
  rewritten to the page id (`rewrite::obsidian_fixups`); other targets, such
  as `LICENSE` or a note naming no page, are left as written. Only a `#`
  that starts a tag with a digit is escaped; letter tags (`#include`) in
  prose still become tags. See the README, Obsidian and Quartz.
- **Old pages keep the old format until recompiled.** Pages compiled by
  0.6.7 or earlier have `## Metadata` bullets and an unfenced code body. Readers accept
  both; a compile by a different tool version re-renders every page.
- **The not-compiled note covers relative paths under pruned folders only.**
  An absolute path, a gitignored path or a hidden file (`.env`) keeps the
  closest-ids text.
- **The not-compiled note is conservative about the project's own name.** When
  the compile input's folder name appears in the typed path at or after the
  pruned folder, the error keeps the closest-ids text.

## `serve --source`

- Files written only through `mmap` are not seen as changed.
- Barrier files left by a crashed server stay in `.wiki/.serve/` until a
  writer's rescan removes them, 10 s on.
- A compile already running when `serve`'s binary is replaced still finishes.
- `serve` sees a binary replaced only at a tool call or before a recompile,
  and `touch` on the binary counts as a replacement.
- `lint` and the `wiki://index` and `wiki://llms.txt` resources read disk,
  not the live snapshot.
- A generated file ignored at file level still triggers a recompile when
  it sits in a watched directory.
- inotify watches `target/` and `.git` too: check the project's directory
  count against `fs.inotify.max_user_watches`.
- Every server on one `--dir`, readers too, watches the whole `--source`
  recursively: N servers hold N copies of those watches.
- Servers from 0.6.2 or older do not take `writer.lock` and still race.
  Restart old sessions after an upgrade.
- Servers on one `--dir` are assumed to share inputs (`--source`,
  `--memory`). If they differ, the writer's inputs win.
- A reader's reload waits on `compile.lock`: during a long compile its call
  can take longer than the usual 5 s bound.
- A reader can load the output of a later compile that failed part way,
  under the earlier ack's empty stale reason. The window is narrow; a plain
  `serve` without `--source` has the same exposure.
- Plain `serve` started while a compile holds `compile.lock`, with no
  `index.json` yet, answers from an empty wiki with no stale line until
  `index.json` appears.
- Orphan cleanup compares file mtimes with the local clock: on NFS, a
  server whose clock runs more than 10 s ahead can remove live barriers or
  acks, and that call waits out its 5 s or reports a read error.
- If the output dir is deleted (`rm -rf`) while a server runs, the old
  writer keeps its lock on the deleted `writer.lock`, so a newly started
  server can become a second writer until the old one restarts.

## `wiki setup`

- Agents: Claude Code, pi, Cursor and Codex. Cursor is detected only by a `.cursor/` folder and
  needs its approval after setup; Codex is set up only with `--agent codex`, and its server entry is
  global (`~/.codex/config.toml`). The Cursor editor is not checked, only `cursor-agent`.
- With `--agent codex`, a project that has `CLAUDE.md` but no `AGENTS.md` gets Codex's server but
  not its navigation steps (exit 1): a new `AGENTS.md` would make pi stop reading `CLAUDE.md`, and
  one an earlier setup created there with only the steps is removed. Seen
  with pi 1.4.0: with both files present, each naming a different codeword, it saw only the
  `AGENTS.md` one.
- Local scope skips (exit 1) a file git already tracks.
- `--remove` leaves `.wiki/` and the ignore lines in place.

## Agent memory

- A plain `wiki compile . .wiki` drops the agents' notes until the server
  recompiles, so `wiki hint` loses the memory line meanwhile.
- `--no-ignore` walks `.wiki/`, whose pages can hold memory text.
- `supersedes:` is not checked for cycles: two notes that supersede each other are both marked superseded.
- Claude Code normalises the project path to NFC before slugging; the wiki
  slugs the path as the OS gives it, so a path with decomposed characters may
  give another slug. `--memory DIR` is the escape.
- Claude Code's overrides are not honoured: the `autoMemoryDirectory` setting,
  `CLAUDE_CODE_REMOTE_MEMORY_DIR`, `CLAUDE_COWORK_MEMORY_PATH_OVERRIDE` and
  `CLAUDE_CODE_PROJECT_DIR_NAME`. `--memory DIR` is the escape.
- A `projects/<slug>/` directory that is missing when `serve` starts is not
  watched; restart the server once it exists.
- Windows slugging (`C:/` paths, the `\\?\` prefix) is covered by unit tests only;
  CI is Linux.
- The tail's note scores share the project's idf scale (statistics over project
  pages plus notes) while the code list keeps its own, notes left out; the two
  scales differ by the notes' share of the corpus, and under a `kind` filter
  also by the docs' share: the tail's note pass runs with `kind: None` while
  the code list honours the caller's `kind` (the hint's note pass uses
  `KindFilter::Code`). Measured trade-off at a gate of 8: on
  this repo's real notes it raised code-query noise from 2 to 6 of 60, while
  recall over all three measured corpora rose from 75 to 80 of 114.
- The gate length is one number for every model, 4, chosen for Claude Haiku's
  short code-shaped queries; at 4 most code queries carry one or two tail rows
  (about 540 bytes more per `search` answer on this repo) that a model writing
  longer queries would not need. The code list is unchanged either way.
  `wiki hint` turns the name branch off past 64 distinct words: a long prompt
  nearly always carries a four-letter word naming some note (a note under 44
  of 48 long prompts, about 1 in 50 relevant). The number is a measured gap
  (questions and bench prompts up to 48 words, long task prompts from 116),
  not a property of prompts in general; a question longer than that shows a
  note only when it outscores the code twice over.
- A `--memory DIR` that does not exist yet, under a large parent (a home
  directory), is watched through the whole parent; on Linux this can exhaust
  `max_user_watches` and `serve` then fails to start. The escape is an
  existing folder.
- `HOME` is read before `USERPROFILE` when finding the config dir.
- A note's author is the author name of the last commit to its file, which git
  lets anyone set: it shows who to ask, it is not authentication.
- An old `index.json` misreads your own committed notes as a co-author's
  until the next compile, so the tail puts them last: without `origin`, every
  relative-path note counts as a co-author's committed note; with `origin` but
  without `mine`, your own committed notes do.
- Trust order only reaches agents through the wiki: an agent that greps
  `.agents/memory` directly still reads a co-author's note the wiki ranks
  low.
- "You" is the commit author email matching `git config user.email`, which
  alone is read (an `author.email` setting or a `GIT_AUTHOR_EMAIL` override is
  not recognised); git lets anyone set an author email, so it shows who to
  ask, not proof.
- A commit that leaves a note's file unchanged triggers no recompile, so the
  author on a served row lags until the next compile or change to a note.
- Origin lookup runs `git log` over the memory folder's whole history on every
  compile, so a very long history adds compile time.
- The ratio branch of the gate (a note scoring twice the best code hit) can
  be gamed by a note whose summary is stuffed with common code words; the
  origin label on the row is the defence, nothing filters such a note.
- Shell: tree-sitter-bash folds a `\.` that starts a line after another command into that command, so that `source` import is not captured.
- A `--memory DIR` folder is always labelled `local`, even when it sits inside the repo and co-authors committed it; only `.agents/memory` gets a git origin.
