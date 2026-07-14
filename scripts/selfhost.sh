#!/usr/bin/env bash
# Compile this repository into its own wiki at .wiki/ (self-hosting).
# Re-run after changes; compilation is incremental.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN="$REPO_ROOT/wiki/target/release/wiki"

if [ ! -x "$BIN" ]; then
    echo "release binary missing; building..." >&2
    cargo build --release --manifest-path "$REPO_ROOT/wiki/Cargo.toml"
fi

cd "$REPO_ROOT"
exec "$BIN" compile . .wiki --incremental --emit-json
