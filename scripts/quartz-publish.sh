#!/usr/bin/env bash
# Publish a compiled wiki directory as a Quartz static site (local preview).
set -euo pipefail

QUARTZ_TAG="v4.5.2"
QUARTZ_REPO="https://github.com/jackyzha0/quartz"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="$REPO_ROOT/.quartz-build"
STRIP_AWK="$SCRIPT_DIR/strip-chrome.awk"

usage() {
    echo "Usage: $(basename "$0") <compiled-wiki-dir> [--title \"Site Title\"] [--serve]" >&2
    exit 2
}

WIKI_DIR=""; TITLE=""; SERVE=0
while [ $# -gt 0 ]; do
    case "$1" in
        --title) [ $# -ge 2 ] && [ -n "$2" ] && [ "${2#-}" = "$2" ] || { echo "error: --title needs a value" >&2; usage; }
                 shift; TITLE="$1" ;;
        --serve) SERVE=1 ;;
        -h|--help) usage ;;
        -*) echo "unknown option: $1" >&2; usage ;;
        *) if [ -z "$WIKI_DIR" ]; then WIKI_DIR="$1"; else echo "unexpected arg: $1" >&2; usage; fi ;;
    esac
    shift
done

[ -n "$WIKI_DIR" ] || usage
[ -d "$WIKI_DIR" ] || { echo "error: not a directory: $WIKI_DIR" >&2; exit 1; }
[ -f "$WIKI_DIR/index.md" ] && [ -f "$WIKI_DIR/index.json" ] \
    || { echo "error: $WIKI_DIR lacks index.md or index.json — is it a compiled wiki?" >&2; exit 1; }
for tool in git node npm npx perl; do
    command -v "$tool" > /dev/null || { echo "error: $tool not found on PATH" >&2; exit 1; }
done
WIKI_DIR="$(cd "$WIKI_DIR" && pwd)"
[ -n "$TITLE" ] || TITLE="$(basename "$WIKI_DIR")"

# Memory notes (agent memory compiled in by `serve`, `setup` or `--memory`)
# are private: their pages, and every link line to them, stay out.
MEMORY_PAGES="$(node -e '
    const idx = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    if (!Array.isArray(idx.entries)) process.exit(1);
    for (const e of idx.entries) if (e.memory) console.log(e.page);
' "$WIKI_DIR/index.json")" || { echo "error: cannot read $WIKI_DIR/index.json" >&2; exit 1; }
MEMORY_LINKS="$(mktemp)"; trap 'rm -f "$MEMORY_LINKS"' EXIT
for page in $MEMORY_PAGES; do
    printf '[[%s|\n](%s)\n' "${page%.md}" "$page" >> "$MEMORY_LINKS"
done
[ -z "$MEMORY_PAGES" ] || echo "Skipping $(echo "$MEMORY_PAGES" | wc -l | tr -d ' ') memory note page(s)"

# 1. Quartz install (once; reused on later runs). npm writes
# node_modules/.package-lock.json last, so a failed install is retried.
if [ ! -f "$BUILD_DIR/package.json" ]; then
    echo "Cloning Quartz $QUARTZ_TAG into $BUILD_DIR ..."
    rm -rf "$BUILD_DIR"
    git clone --depth 1 --branch "$QUARTZ_TAG" "$QUARTZ_REPO" "$BUILD_DIR"
fi
[ -f "$BUILD_DIR/node_modules/.package-lock.json" ] || ( cd "$BUILD_DIR" && npm install )

# 2. Patch pageTitle (value-agnostic; " and \ would break the TS string).
SAFE_TITLE="$(printf '%s' "$TITLE" | tr '"\\' "'/")"
TITLE="$SAFE_TITLE" perl -i -pe 's/pageTitle: "[^"]*"/pageTitle: "$ENV{TITLE}"/' "$BUILD_DIR/quartz.config.ts"
grep -Fq "pageTitle: \"$SAFE_TITLE\"" "$BUILD_DIR/quartz.config.ts" \
    || { echo "error: could not set pageTitle in $BUILD_DIR/quartz.config.ts" >&2; exit 1; }

# 3. Sync content: wipe, copy human pages through the strip transform.
CONTENT="$BUILD_DIR/content"
rm -rf "$CONTENT"; mkdir -p "$CONTENT"
shopt -s nullglob
for f in "$WIKI_DIR"/*.md; do
    base="$(basename "$f")"
    [ "$base" = "AGENTS.md" ] && continue
    printf '%s\n' "$MEMORY_PAGES" | grep -Fxq -- "$base" && continue
    awk -f "$STRIP_AWK" "$f" | { grep -vFf "$MEMORY_LINKS" || true; } > "$CONTENT/$base"
done
shopt -u nullglob
SYNCED="$(find "$CONTENT" -name '*.md' | wc -l | tr -d ' ')"
[ "$SYNCED" -gt 0 ] || { echo "error: no pages synced from $WIKI_DIR" >&2; exit 1; }
echo "Synced $SYNCED pages into content/"

# 4. Build (or serve).
cd "$BUILD_DIR"
if [ "$SERVE" -eq 1 ]; then
    echo "Serving at http://localhost:8080 (Ctrl-C to stop) ..."
    npx quartz build --serve
else
    npx quartz build
    echo "Built site → $BUILD_DIR/public (open public/index.html)"
fi
