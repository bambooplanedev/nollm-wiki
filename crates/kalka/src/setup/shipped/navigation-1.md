## Find code with the project wiki

1. Call the wiki `search` tool with the words of what you are looking for and `kind: "code"`.
   Each symbol line is `path:start-end  name  signature`.
2. Read only those lines with your file tool. The start and end lines are exact — do not
   re-derive them with grep.
3. For a module's map, call `page` with the path; it lists every symbol with its lines. To follow
   a function through the code, call `neighbors` with the path and `symbol`. To see what calls a
   function, call `callers` with the path and `symbol` instead of grepping for its name.
4. Only if search finds nothing relevant, try other words, then fall back to grep.
