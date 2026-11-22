Code lookup:
- Where is X defined or handled: `mcp__wiki__search` with `kind: "code"`, first.
- Read only the line ranges it returns.
- Who calls X: `mcp__wiki__callers`.
- grep only literal strings (messages, env vars, config keys), never in the same turn as a search.
