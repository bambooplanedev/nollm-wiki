#!/usr/bin/env bash
# Search-quality eval: top-1 hit rate over fixed queries against the
# self-hosted wiki (.wiki/). Run scripts/selfhost.sh first. Prints "N/M".
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/wiki/target/release/wiki"
DIR="${1:-$ROOT/.wiki}"

# query | expected top-1 page id
CASES='
determinism rules|architecture
incremental cache|cache
broken wikilinks orphans|lint
mcp server stdio|serve
neighbors budget max tokens|src_query
python docstring extraction|extract_python
rust impl methods owner|extract_rust
go exported identifier|extract_simple
quartz static site publish|scripts_quartz_publishing_workflow
content hash|hash
walk source tree sorted|walk
watch recompile on change|watch
wikilink rewrite obsidian|rewrite
manifest|manifest
self-hosting dogfood findings|self_hosting_dogfood_findings_2026_07_14
'

hits=0; total=0
while IFS='|' read -r q want; do
  [ -z "$q" ] && continue
  total=$((total+1))
  got="$("$BIN" search "$q" --dir "$DIR" --limit 1 | head -1 | cut -f1)"
  if [ "$got" = "$want" ]; then hits=$((hits+1)); mark="ok  "; else mark="MISS"; fi
  printf '%s  %-40s want=%-45s got=%s\n' "$mark" "$q" "$want" "${got:-<none>}"
done <<< "$CASES"

echo "$hits/$total"
