#!/usr/bin/env bash
# Test for strip-chrome.awk on pages the current compiler writes (no network).
# Uses $WIKI_BIN when set, else `cargo run` on this repo's kalka crate.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AWK="$SCRIPT_DIR/strip-chrome.awk"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
fail=0
ok()  { echo "  ok: $1"; }
bad() { echo "  FAIL: $1"; fail=1; }
present() { printf '%s\n' "$1" | grep -Fxq -- "$2" && ok "present: $2" || bad "MISSING: $2"; }
absent()  { printf '%s\n' "$1" | grep -Fxq -- "$2" && bad "LEAKED: $2"  || ok "absent: $2"; }
strip()   { awk -f "$AWK" "$tmp/out/$1"; }

# A source tree whose pages get every backlink section, plus a markdown body
# that uses the compiler's headings as its own content.
mkdir -p "$tmp/src"
printf '"""Alpha."""\nimport b\n\ndef alpha():\n    return b.beta()\n' > "$tmp/src/a.py"
printf '"""Beta."""\n\ndef beta():\n    return 1\n' > "$tmp/src/b.py"
# (printf, not a heredoc: a "## " line here would split this script's own
# code page into sections when the repo compiles itself.)
printf '%s\n' '# Guide' '' 'Uses [b](b.py).' '' '## Mentioned in' \
    'Author text under a chrome-like heading.' '' '## Imported by' 'More author text.' '' \
    '## Notes' 'Author notes inside the body.' > "$tmp/src/guide.md"
printf '# Other\n\nSee [guide](guide.md).\n' > "$tmp/src/other.md"
if [ -n "${WIKI_BIN:-}" ]; then
    "$WIKI_BIN" compile "$tmp/src" "$tmp/out" > /dev/null
else
    cargo run --quiet --manifest-path "$SCRIPT_DIR/../Cargo.toml" -p kalka -- compile "$tmp/src" "$tmp/out" > /dev/null
fi
grep -Fxq '## Imported by' "$tmp/out/b.md" && grep -Fxq '## Mentioned in' "$tmp/out/b.md" \
    || { echo "fixture: b.md lacks the backlink sections; update this test"; exit 1; }

# Backlinks go; metadata, outgoing links, exports and body stay.
out="$(strip b.md)"
absent  "$out" "## Imported by"
absent  "$out" "- [[a|A]]"
absent  "$out" "## Mentioned in"
absent  "$out" "- [[guide|Guide]]"
absent  "$out" "## Notes"
absent  "$out" "_(add your own notes here — preserved on recompile)_"
present "$out" "kind: code:python"
present "$out" "## Exports"
present "$out" "def beta():"
present "$(strip a.md)" "- [[b|B]]"

# The same headings inside a markdown body are author text.
[ "$(grep -Fxc '## Mentioned in' "$tmp/out/guide.md")" -eq 2 ] \
    || { echo "fixture: guide.md lacks its compiler Mentioned in section; update this test"; exit 1; }
out="$(strip guide.md)"
absent  "$out" "- [[other|Other]]"
present "$out" "## Mentioned in"
present "$out" "Author text under a chrome-like heading."
present "$out" "## Imported by"
present "$out" "More author text."
present "$out" "Author notes inside the body."
absent  "$out" "_(add your own notes here — preserved on recompile)_"

# Hand-written notes are not published; the body's own Notes heading is.
sed 's/^_(add your own notes here.*/my hand note/' "$tmp/out/guide.md" > "$tmp/out/noted.md"
out="$(strip noted.md)"
absent  "$out" "my hand note"
[ "$(printf '%s\n' "$out" | grep -Fxc '## Notes')" -eq 1 ] && ok "one Notes heading left" || bad "trailing Notes kept"
present "$out" "Author notes inside the body."

# Idempotent (on a page whose body has no Notes heading).
once="$(strip b.md)"
[ "$once" = "$(printf '%s\n' "$once" | awk -f "$AWK")" ] && ok "idempotent" || bad "not idempotent"

if [ "$fail" -eq 0 ]; then echo "strip-chrome: ALL PASS"; else echo "strip-chrome: FAILURES"; exit 1; fi
