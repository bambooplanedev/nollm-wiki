#!/bin/sh
# release-assemble.sh: writes SHA256SUMS and manifest.json beside a release's archives.
#
#   sh scripts/release-assemble.sh <version> <dir> [--tree <out>]
#
# <dir> holds one <binary>-v<version>-<target>.tar.gz per binary and target, each with the bare
# binary as its one member. The script checks them all, then writes <dir>/SHA256SUMS, and
# <dir>/manifest.json from release/modules.json, and copies the repository's install.sh into
# <dir>. With --tree it also lays out <out> as NOLLM_WIKI_RELEASE_URL=file://<out> expects:
# <out>/download/v<version>/ (archives, SHA256SUMS, manifest.json, install.sh) and
# <out>/latest/download/ (SHA256SUMS, manifest.json, install.sh), and SHA256SUMS.minisig and
# manifest.json.minisig at both places when <dir> holds them (this script makes no signature).
# <out> must not exist. The three files reach <dir> only after the tree is built. Exit 0, or 1
# with a line naming what is wrong; nothing is written unless every check passes.
#
# Needs jq, tar, and sha256sum or shasum.

set -eu

targets='aarch64-apple-darwin x86_64-unknown-linux-musl'
binaries='wiki plaz nollm-wiki-install'

die() {
    printf 'release-assemble.sh: %s\n' "$1" >&2
    exit 1
}

[ "$#" = 2 ] || { [ "$#" = 4 ] && [ "$3" = --tree ]; } ||
    die "usage: release-assemble.sh <version> <dir> [--tree <out>]"
version=$1
dir=$2
tree=${4:-}

# X.Y.Z, each a number with no leading zero (but 0), as nollm-wiki-install reads it.
case $version in
    *[!0-9.]*) die "bad version: $version" ;;
esac
printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]{0,18})(\.(0|[1-9][0-9]{0,18})){2}$' ||
    die "bad version: $version"

for tool in jq tar; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
done
if command -v sha256sum >/dev/null 2>&1; then
    sha() { sha256sum <"$1"; }
elif command -v shasum >/dev/null 2>&1; then
    sha() { shasum -a 256 <"$1"; }
else
    die "sha256sum or shasum is required"
fi

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
modules=$repo/release/modules.json
[ -d "$dir" ] || die "$dir is not a directory"
[ -w "$dir" ] || die "$dir is not writable"
if [ -n "$tree" ] && { [ -e "$tree" ] || [ -L "$tree" ]; }; then
    die "$tree already exists"
fi
sh -n "$repo/install.sh" || die "install.sh does not parse"

# Every module of modules.json is a binary, and every binary but nollm-wiki-install is a module.
names=$(jq -r 'if type == "array" and all(.[]; type == "object" and (.name | type) == "string")
    then .[].name else error("not an array of named modules") end' "$modules") ||
    die "$modules is not a list of modules"
want=$(for b in $binaries; do [ "$b" = nollm-wiki-install ] || printf '%s\n' "$b"; done | sort)
[ "$(printf '%s\n' "$names" | sort)" = "$want" ] ||
    die "$modules names $(printf '%s\n' "$names" | tr '\n' ' ')but the binaries are $binaries"

# Each archive holds exactly one member, a regular file with the bare binary name.
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
for b in $binaries; do
    for t in $targets; do
        f=$b-v$version-$t.tar.gz
        { [ -f "$dir/$f" ] && [ ! -L "$dir/$f" ]; } || die "$f is missing from $dir"
        members=$(tar -tzf "$dir/$f") || die "$f is not a readable .tar.gz"
        [ "$members" = "$b" ] || die "$f must hold only $b; it holds: $members"
        mkdir -- "$scratch/$f"
        tar -xzf "$dir/$f" -C "$scratch/$f" || die "$f cannot be unpacked"
        { [ -f "$scratch/$f/$b" ] && [ ! -L "$scratch/$f/$b" ]; } ||
            die "$f: $b is not a regular file"
    done
done

# Checked: write into the scratch directory; <dir> gets the files last, after the tree is built.
: >"$scratch/SHA256SUMS"
for b in $binaries; do
    for t in $targets; do
        f=$b-v$version-$t.tar.gz
        sum=$(sha "$dir/$f")
        printf '%s  %s\n' "${sum%% *}" "$f" >>"$scratch/SHA256SUMS"
    done
done

jq --arg v "$version" --rawfile sums "$scratch/SHA256SUMS" '
    def archives($b): ($b + "-v" + $v + "-") as $p
        | [$sums | split("\n")[] | select(. != "") | split("  ")
           | {file: .[1], sha256: .[0]} | select(.file | startswith($p))
           | {key: (.file | ltrimstr($p) | rtrimstr(".tar.gz")), value: .}]
        | from_entries;
    {version: $v, modules: map(. + {archives: archives(.name)}),
     "nollm-wiki-install": {archives: archives("nollm-wiki-install")}}
' "$modules" >"$scratch/manifest.json"

cp -- "$repo/install.sh" "$scratch/install.sh"

if [ -n "$tree" ]; then
    mkdir -p -- "$tree/download/v$version" "$tree/latest/download"
    for b in $binaries; do
        for t in $targets; do
            cp -- "$dir/$b-v$version-$t.tar.gz" "$tree/download/v$version/"
        done
    done
    for at in "download/v$version" latest/download; do
        cp -- "$scratch/SHA256SUMS" "$scratch/manifest.json" "$scratch/install.sh" "$tree/$at/"
        for f in SHA256SUMS.minisig manifest.json.minisig; do
            [ ! -e "$dir/$f" ] || cp -- "$dir/$f" "$tree/$at/"
        done
    done
fi

# Last: nothing above can fail any more.
mv -- "$scratch/SHA256SUMS" "$scratch/manifest.json" "$scratch/install.sh" "$dir/"
