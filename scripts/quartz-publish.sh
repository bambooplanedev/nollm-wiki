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
        --title) shift; TITLE="${1:-}" ;;
        --serve) SERVE=1 ;;
        -h|--help) usage ;;
        -*) echo "unknown option: $1" >&2; usage ;;
        *) if [ -z "$WIKI_DIR" ]; then WIKI_DIR="$1"; else echo "unexpected arg: $1" >&2; usage; fi ;;
    esac
    shift
done

[ -n "$WIKI_DIR" ] || usage
[ -d "$WIKI_DIR" ] || { echo "error: not a directory: $WIKI_DIR" >&2; exit 1; }
[ -f "$WIKI_DIR/index.md" ] || { echo "error: $WIKI_DIR has no index.md — is it a compiled wiki?" >&2; exit 1; }
WIKI_DIR="$(cd "$WIKI_DIR" && pwd)"
[ -n "$TITLE" ] || TITLE="$(basename "$WIKI_DIR")"

# 1. Quartz install (once; reused on later runs).
if [ ! -f "$BUILD_DIR/package.json" ]; then
    echo "Cloning Quartz $QUARTZ_TAG into $BUILD_DIR ..."
    rm -rf "$BUILD_DIR"
    git clone --depth 1 --branch "$QUARTZ_TAG" "$QUARTZ_REPO" "$BUILD_DIR"
    ( cd "$BUILD_DIR" && npm install )
fi

# 2. Patch pageTitle (value-agnostic; sanitize any double-quotes in the title).
SAFE_TITLE="${TITLE//\"/\'}"
TITLE="$SAFE_TITLE" perl -i -pe 's/pageTitle: "[^"]*"/pageTitle: "$ENV{TITLE}"/' "$BUILD_DIR/quartz.config.ts"

# 3. Sync content: wipe, copy human pages through the strip transform.
CONTENT="$BUILD_DIR/content"
rm -rf "$CONTENT"; mkdir -p "$CONTENT"
shopt -s nullglob
for f in "$WIKI_DIR"/*.md; do
    base="$(basename "$f")"
    [ "$base" = "AGENTS.md" ] && continue
    awk -f "$STRIP_AWK" "$f" > "$CONTENT/$base"
done
shopt -u nullglob
echo "Synced $(find "$CONTENT" -name '*.md' | wc -l | tr -d ' ') pages into content/"

# 4. Build (or serve).
cd "$BUILD_DIR"
if [ "$SERVE" -eq 1 ]; then
    echo "Serving at http://localhost:8080 (Ctrl-C to stop) ..."
    npx quartz build --serve
else
    npx quartz build
    echo "Built site → $BUILD_DIR/public (open public/index.html)"
fi
