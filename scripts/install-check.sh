#!/bin/sh
# install-check.sh: installs a release the way a user does, in a temp HOME, and checks it works.
#
#   sh scripts/install-check.sh <version> [<tree>]
#
# <tree> (default /release) is what `release-assemble.sh --tree` lays out. curl and git are
# installed first when missing (apk or apt-get). Exit 0, or 1 with a line naming what failed.
set -eu
version=$1
tree=$(CDPATH='' cd -- "${2:-/release}" && pwd)
fail() { printf 'install-check.sh: %s\n' "$1" >&2; exit 1; }

if ! command -v curl >/dev/null || ! command -v git >/dev/null; then
    if command -v apk >/dev/null; then apk add --no-cache -q curl git
    else apt-get update -qq && apt-get install -y -qq --no-install-recommends curl git >/dev/null; fi
fi

tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
HOME=$tmp/home NOLLM_WIKI_HOME=$tmp/home/.nollm-wiki-home NOLLM_WIKI_RELEASE_URL=file://$tree
export HOME NOLLM_WIKI_HOME NOLLM_WIKI_RELEASE_URL
unset CLAUDE_CONFIG_DIR XDG_CONFIG_HOME
mkdir -- "$HOME" "$tmp/project"
bin=$NOLLM_WIKI_HOME/bin

# shellcheck disable=SC2002 # piped, as the one-line install does
cat "$tree/latest/download/install.sh" | sh
out=$("$bin/nollm-wiki" install --yes) || fail "nollm-wiki install --yes failed"
printf '%s\n' "$out"
printf '%s\n' "$out" | grep -qxF "$bin is not on PATH; to type nollm-wiki in a terminal, add it:" || fail "no 'is not on PATH' line"
printf '%s\n' "$out" | grep -qxF "    export PATH=\"$bin:\$PATH\"" || fail "no 'export PATH' line"
case $("$bin/nollm-wiki") in "nollm-wiki $version in "*) ;; *) fail "nollm-wiki does not show $version" ;; esac
for b in nollm-wiki wiki plaz; do
    [ "$("$bin/$b" --version)" = "$b $version" ] || fail "$b --version is not $b $version"
done
cd -- "$tmp/project" && git init -q
printf 'def f(x):\n    if x:\n        return 1\n    return 2\n' >f.py
code=0 && "$bin/nollm-wiki" setup </dev/null || code=$?
[ "$code" = 2 ] || fail "nollm-wiki setup without --scope and a terminal exited $code, not 2"
"$bin/nollm-wiki" setup --scope repo --yes </dev/null || fail "nollm-wiki setup --scope repo failed"
[ -f .nollm-wiki/index.md ] || fail "setup made no .nollm-wiki/index.md"
[ -d .nollm-wiki/map ] || fail "setup made no .nollm-wiki/map/"
printf 'install-check.sh: %s on %s/%s: ok\n' "$version" "$(uname -s)" "$(uname -m)"
