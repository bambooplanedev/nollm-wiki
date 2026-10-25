---
name: wiki-navigation
description: Find where something is or how it works in this project's code. Use before grep, find or reading whole files, whenever the task needs a function, type, handler or module location.
---

# Find code with the project wiki

1. Call the wiki `search` tool (`mcp__wiki__search`) with the words of
   what you are looking for and `kind: "code"`. Each symbol line is `path:start-end  name  signature`.
2. Read only those lines with your file tool. The start and end lines are
   exact — do not re-derive them with grep.
3. For a module's map, call `page` (`mcp__wiki__page`) with the path; it lists every symbol
   with its lines. To follow a function through the code, call `neighbors`
   (`mcp__wiki__neighbors`) with the path and `symbol`: it lists every
   function that one calls, with its lines. To see what calls a function,
   call `callers` (`mcp__wiki__callers`) with the path and `symbol`
   instead of grepping for its name.
4. Only if search finds nothing relevant, try other words (a name, an
   abbreviation), then fall back to grep.

# Memory

Notes about this project live in `<memory-dir>`; `search` finds them before
code.

- Write one note per fact or topic, named for what it says.
- A note not written in English needs English `aliases:` in its front matter
  (search words are English).
- A journal note is named `YYYY-MM-DD.md` and has the date in its H1.
- Search first; update the note that exists instead of adding a copy.
- When a decision changes, update its note. If you write a new note instead, add `supersedes: <old note's title>` to it.
