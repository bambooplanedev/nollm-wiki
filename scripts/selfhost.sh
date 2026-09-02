#!/usr/bin/env bash
# Compile this repository into its own wiki at .wiki/ (self-hosting).
# Re-run after changes; compilation is incremental.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN="$REPO_ROOT/wiki/target/release/wiki"

# Always build: cargo is a no-op when nothing changed, and this guarantees the
# binary reflects the current sources rather than silently serving a stale one.
cargo build --release --manifest-path "$REPO_ROOT/wiki/Cargo.toml"

cd "$REPO_ROOT"
"$BIN" compile . .wiki --incremental --emit-json
# Gate: a broken wikilink in the self-hosted wiki fails the script (exit 1).
"$BIN" lint --dir .wiki
