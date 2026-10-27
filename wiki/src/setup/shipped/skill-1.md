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
   with its lines.
4. Only if search finds nothing relevant, try other words (a name, an
   abbreviation), then fall back to grep.
