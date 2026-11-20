#!/bin/sh
# release-sign.sh: signs the draft release of <version> on the owner's machine.
#
#   sh scripts/release-sign.sh <version> [--key <secret key file>] [--replace]
#
# Run it in a terminal, inside this checkout: gh takes the repository from it, and minisign asks
# for the secret key's password on the terminal, once per file. The script reads the draft
# v<version> (it must be under the releases address of crates/nollm-wiki-install/src/install.rs, so a fork's
# draft is refused, and hold exactly the 9 files the release workflow uploads), downloads it into a
# temp dir, checks every archive's SHA-256 against SHA256SUMS and manifest.json, the manifest's
# version, and install.sh against the tag's, signs manifest.json and SHA256SUMS (minisign -S;
# --key is passed as -s, otherwise minisign's default key is used), verifies both signatures with
# release/nollm-wiki.pub and uploads the two .minisig files to the draft. With --replace, signatures
# already on the draft are replaced; without it they stop the script. It never publishes: it ends
# with the command to. It never reads, copies or prints the secret key, and never sees the
# password. Exit 0, or 1 with a line naming what is wrong; nothing is uploaded unless every check
# passes.
#
# Needs gh, minisign, jq, git, and sha256sum or shasum.
#
# NOLLM_WIKI_SIGN_NO_TTY_CHECK=1 skips the terminal check, for the tests only (they run a fake
# minisign); the real minisign still needs the terminal for the password.

set -eu
LC_ALL=C
export LC_ALL

targets='aarch64-apple-darwin x86_64-unknown-linux-musl'
binaries='wiki plaz nollm-wiki-install'

die() {
    printf 'release-sign.sh: %s\n' "$1" >&2
    exit 1
}

usage="usage: release-sign.sh <version> [--key <secret key file>] [--replace]"
[ "$#" -ge 1 ] || die "$usage"
version=$1
shift
key=
replace=
while [ "$#" -gt 0 ]; do
    case $1 in
        --key)
            { [ "$#" -ge 2 ] && [ -n "$2" ]; } || die "$usage"
            key=$2
            shift 2
            ;;
        --replace)
            replace=yes
            shift
            ;;
        *) die "$usage" ;;
    esac
done

# X.Y.Z, each a number with no leading zero (but 0), as nollm-wiki-install reads it.
case $version in
    *[!0-9.]*) die "bad version: $version" ;;
esac
printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]{0,18})(\.(0|[1-9][0-9]{0,18})){2}$' ||
    die "bad version: $version"
tag=v$version

if [ "${NOLLM_WIKI_SIGN_NO_TTY_CHECK:-}" != 1 ] && [ ! -t 0 ]; then
    die "run it in a terminal: minisign asks there for the secret key's password"
fi

for tool in gh minisign jq git; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
done
if command -v sha256sum >/dev/null 2>&1; then
    sha() { sha256sum <"$1"; }
elif command -v shasum >/dev/null 2>&1; then
    sha() { shasum -a 256 <"$1"; }
else
    die "sha256sum or shasum is required"
fi

# The key path as given, from where the script was started; then everything runs in the checkout.
case $key in
    '' | /*) ;;
    *) key=$(pwd)/$key ;;
esac
repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd -- "$repo"
pub=release/nollm-wiki.pub
# The releases address the installer uses (install.rs), so a draft of a fork is never signed.
extract='s|^const RELEASES: &str = "\(https://github\.com/[^/"]*/[^/"]*/releases\)";$|\1|p'
releases=$(sed -n "$extract" crates/nollm-wiki-install/src/install.rs 2>/dev/null) || :
case $releases in
    https://*/releases) ;;
    *) die "no releases address in crates/nollm-wiki-install/src/install.rs" ;;
esac
keyid=$(sed -n '1s/^untrusted comment: minisign public key //p' "$pub" 2>/dev/null) || :
[ -n "$keyid" ] || die "$pub is not a minisign public key file"

tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

archives=$(for b in $binaries; do
    for t in $targets; do printf '%s\n' "$b-v$version-$t.tar.gz"; done
done | sort)
printf '%s\n' "$archives" SHA256SUMS manifest.json install.sh | sort >"$tmp/want"

# The draft's files: exactly the 9, or with --replace also the signatures.
# $1: what is checked, for the message.
assets() {
    gh release view "$tag" --json url,isDraft,assets >"$tmp/view.json" ||
        die "cannot read the release $tag ($1); is the tag pushed and the draft made?"
    url=$(jq -r '.url' "$tmp/view.json") || die "gh gave no release for $tag"
    case $url in
        "$releases"/*) ;;
        *) die "this draft belongs to $url, not to $releases" ;;
    esac
    draft=$(jq -r '.isDraft' "$tmp/view.json") || die "gh gave no release for $tag"
    case $draft in
        true) ;;
        false) die "$tag is already published; only a draft is signed" ;;
        *) die "could not read the release's draft state" ;;
    esac
    jq -r '.assets[].name' "$tmp/view.json" >"$tmp/names" || die "gh gave no files for $tag"
    sort "$tmp/names" >"$tmp/got"
}
list() { tr '\n' ' ' <"$1" | sed 's/ $//'; }

assets "before signing"
comm -23 "$tmp/want" "$tmp/got" >"$tmp/missing"
comm -13 "$tmp/want" "$tmp/got" >"$tmp/extra"
printf '%s\n' SHA256SUMS.minisig manifest.json.minisig >"$tmp/sigs"
comm -23 "$tmp/extra" "$tmp/sigs" >"$tmp/unexpected"
[ ! -s "$tmp/missing" ] || die "$tag lacks: $(list "$tmp/missing")"
[ ! -s "$tmp/unexpected" ] || die "$tag holds unexpected files: $(list "$tmp/unexpected")"
if [ -s "$tmp/extra" ] && [ -z "$replace" ]; then
    die "$tag is already signed ($(list "$tmp/extra")); --replace signs it again"
fi

dl=$tmp/release
gh release download "$tag" --dir "$dl" || die "cannot download the release $tag"
(cd -- "$dl" && ls -A) | sort >"$tmp/downloaded"
cmp -s "$tmp/got" "$tmp/downloaded" || die "the download of $tag is not the release's file list"
rm -f -- "$dl/SHA256SUMS.minisig" "$dl/manifest.json.minisig"

# SHA256SUMS lists the 6 archives, each once; manifest.json is of this version and records the
# same 6; each archive's SHA-256 is the one both state.
sums=$dl/SHA256SUMS
manifest=$dl/manifest.json
{ [ "$(grep -c '' "$sums")" = 6 ] &&
    [ "$(sed -n 's/^[0-9a-f]\{64\}  //p' "$sums" | sort)" = "$archives" ]; } ||
    die "SHA256SUMS does not list exactly the 6 archives of $version"
stated=$(jq -r '.version' "$manifest") || die "manifest.json cannot be read"
[ "$stated" = "$version" ] || die "manifest.json states version $stated, not $version"
n=$(jq -r '[.modules[].archives[], .["nollm-wiki-install"].archives[]] | length' "$manifest") ||
    die "manifest.json does not list archives as nollm-wiki-install reads them"
[ "$n" = 6 ] || die "manifest.json lists $n archives, not 6"
for f in $archives; do
    sum=$(sha "$dl/$f")
    sum=${sum%% *}
    [ "$(awk -v f="$f" '$2 == f { print $1 }' "$sums")" = "$sum" ] ||
        die "$f: its SHA-256 is not the one in SHA256SUMS"
    in_manifest=$(jq -r --arg f "$f" '[.modules[].archives[], .["nollm-wiki-install"].archives[]]
        | map(select(.file == $f) | .sha256) | if length == 1 then .[0] else "" end' "$manifest")
    [ "$in_manifest" = "$sum" ] || die "$f: its SHA-256 is not the one in manifest.json"
done
git rev-parse -q --verify "refs/tags/$tag^{commit}" >/dev/null ||
    die "the tag $tag is not in this checkout; run git fetch --tags"
git show "refs/tags/$tag:install.sh" >"$tmp/install.sh" || die "$tag has no install.sh"
cmp -s "$tmp/install.sh" "$dl/install.sh" || die "install.sh of the draft is not the one at $tag"

repository=${releases#https://github.com/}
printf 'repository: %s\n' "${repository%/releases}"
printf 'release: %s\n' "$url"
printf 'signing the draft %s:\n' "$tag"
for f in $archives; do
    printf '  %s  %s\n' "$(awk -v f="$f" '$2 == f { print $1 }' "$sums")" "$f"
done
printf 'with the key %s (%s)\n' "$keyid" "$pub"
printf 'minisign asks for the password twice, once per file\n'

for f in manifest.json SHA256SUMS; do
    if [ -n "$key" ]; then
        minisign -S -s "$key" -m "$dl/$f" -t "nollm-wiki-install $version $f" || die "$f: signing failed"
    else
        minisign -S -m "$dl/$f" -t "nollm-wiki-install $version $f" || die "$f: signing failed"
    fi
done
# -H: only the prehashed form, the one nollm-wiki-install takes.
for f in manifest.json SHA256SUMS; do
    minisign -V -H -q -p "$pub" -m "$dl/$f" -x "$dl/$f.minisig" ||
        die "$f: the new signature does not verify with $pub (signed with another key?);\
 nothing uploaded"
done

if [ -n "$replace" ]; then
    gh release upload "$tag" "$dl/manifest.json.minisig" "$dl/SHA256SUMS.minisig" --clobber ||
        die "uploading the signatures to $tag failed"
else
    gh release upload "$tag" "$dl/manifest.json.minisig" "$dl/SHA256SUMS.minisig" ||
        die "uploading the signatures to $tag failed"
fi

assets "after the upload"
cat "$tmp/want" "$tmp/sigs" | sort >"$tmp/signed"
cmp -s "$tmp/signed" "$tmp/got" ||
    die "$tag does not hold the 11 files after the upload: $(list "$tmp/got")"
printf '%s is signed and still a draft (11 files). To publish it:\n' "$tag"
printf '    gh release edit %s --draft=false\n' "$tag"
printf 'Published, it is the latest release: the one install.sh installs.\n'
