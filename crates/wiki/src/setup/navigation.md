## Find code with the project wiki

1. Call the wiki `search` tool with the words of what you are looking for and `kind: "code"`.
   Each symbol line is `path:start-end  name  signature`.
2. Read only those lines with your file tool. The start and end lines are exact — do not
   re-derive them with grep.
3. Look up names you already know with `search`; put several in one query. Use `page` only to
   see every symbol of a module you have not seen. To see what calls a function, call `callers`
   with the path and `symbol` instead of grepping for its name.
4. Do not call grep, find or ls in the same turn as `search`. Wait for the `search` answer and
   read it first; most of the time it already holds what you need.
5. Pick the tool by the question: where something is defined or handled, `search`; who calls a
   function, `callers`; every occurrence of a literal string (an env var, a message, a config key),
   grep.
6. Only if search finds nothing relevant, try other words, then fall back to grep.
