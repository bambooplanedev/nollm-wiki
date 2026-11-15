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
HOME=$tmp/home KULMAN_HOME=$tmp/home/.kulman-home KULMAN_RELEASE_URL=file://$tree
export HOME KULMAN_HOME KULMAN_RELEASE_URL
unset CLAUDE_CONFIG_DIR XDG_CONFIG_HOME
mkdir -- "$HOME" "$tmp/project"
bin=$KULMAN_HOME/bin

# shellcheck disable=SC2002 # piped, as the one-line install does
cat "$tree/latest/download/install.sh" | sh
out=$("$bin/kulman" install --yes) || fail "kulman install --yes failed"
printf '%s\n' "$out"
printf '%s\n' "$out" | grep -qxF "$bin is not on PATH; add it:" || fail "no 'is not on PATH' line"
printf '%s\n' "$out" | grep -qxF "    export PATH=\"$bin:\$PATH\"" || fail "no 'export PATH' line"
case $("$bin/kulman") in "kulman $version in "*) ;; *) fail "kulman does not show $version" ;; esac
for b in kulman calque plaz; do
    [ "$("$bin/$b" --version)" = "$b $version" ] || fail "$b --version is not $b $version"
done
cd -- "$tmp/project" && git init -q
printf 'def f(x):\n    if x:\n        return 1\n    return 2\n' >f.py
code=0 && "$bin/kulman" setup </dev/null || code=$?
[ "$code" = 2 ] || fail "kulman setup without --scope and a terminal exited $code, not 2"
"$bin/kulman" setup --scope repo --yes </dev/null || fail "kulman setup --scope repo failed"
[ -f .kulman/index.md ] || fail "setup made no .kulman/index.md"
[ -d .kulman/map ] || fail "setup made no .kulman/map/"
printf 'install-check.sh: %s on %s/%s: ok\n' "$version" "$(uname -s)" "$(uname -m)"
