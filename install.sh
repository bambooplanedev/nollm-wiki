#!/bin/sh
# install.sh: puts the kulman binary of the latest release into ~/.kulman-home/bin.
#
#   curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/kulman/releases/latest/download/install.sh | sh
#
# It reads SHA256SUMS of the latest release, downloads the kulman archive for this machine,
# checks it against that file, and moves the one binary to <home>/bin/kulman. It writes nothing
# else and edits no shell profile; `kulman install` then installs the modules. With minisign
# installed it first checks SHA256SUMS.minisig against the release key (release/kulman.pub).
#
# KULMAN_HOME     the home directory (default: $HOME/.kulman-home)
# KULMAN_RELEASE_URL  where releases are (default: https://github.com/bambooplanedev/kulman/releases)
#
# Everything is inside main, called on the last line: a download cut off midway runs nothing.

set -eu

main() {
    LC_ALL=C
    export LC_ALL

    # The key line of release/kulman.pub.
    ki_key=RWSDr1EMmW7Mh0fB1YXLI+poqKnCnGO9UTdaKQ8QmEm/t4rE6kY1dvZr
    ki_base=${KULMAN_RELEASE_URL:-https://github.com/bambooplanedev/kulman/releases}
    while :; do
        case $ki_base in
            */) ki_base=${ki_base%/} ;;
            *) break ;;
        esac
    done
    case $ki_base in
        *'?'* | *'#'*) ki_https=bad ;;
        https://?*) ki_https=yes ;;
        file:///* | file://localhost/*) ki_https= ;;
        *) ki_https=bad ;;
    esac
    if [ "$ki_https" = bad ]; then
        printf 'install.sh: KULMAN_RELEASE_URL must start with https:// or file:// (got %s)\n' \
            "$ki_base" >&2
        exit 2
    fi

    if [ -n "${KULMAN_HOME:-}" ]; then
        ki_home=$KULMAN_HOME
    elif [ -n "${HOME:-}" ]; then
        ki_home=$HOME/.kulman-home
    else
        die "set HOME or KULMAN_HOME"
    fi
    case $ki_home in
        /*) ;;
        *) ki_home=$(pwd)/$ki_home ;;
    esac
    while :; do
        case $ki_home in
            ?*/) ki_home=${ki_home%/} ;;
            *) break ;;
        esac
    done

    ki_os=$(uname -s)
    ki_arch=$(uname -m)
    case $ki_os/$ki_arch in
        Darwin/arm64) ki_target=aarch64-apple-darwin ;;
        Linux/x86_64) ki_target=x86_64-unknown-linux-musl ;;
        *) die "no build for $ki_os/$ki_arch" ;;
    esac

    for ki_tool in curl tar mktemp; do
        command -v "$ki_tool" >/dev/null 2>&1 || die "$ki_tool is required"
    done
    if command -v sha256sum >/dev/null 2>&1; then
        ki_sha=sha256sum
    elif command -v shasum >/dev/null 2>&1; then
        ki_sha=shasum
    else
        die "sha256sum or shasum is required"
    fi

    ki_parent=$(dirname -- "$ki_home")
    [ -d "$ki_parent" ] || die "$ki_parent does not exist"
    ki_bin=$ki_home/bin
    ki_dest=$ki_bin/kulman
    if [ -L "$ki_bin" ] || { [ -e "$ki_bin" ] && [ ! -d "$ki_bin" ]; }; then
        die "$ki_bin is not a directory kulman can use"
    fi
    # mv onto a directory would move the binary into it.
    [ ! -d "$ki_dest" ] || die "$ki_dest is not a file"

    # From here on, cleanup removes the temp dir, and bin/ and home if this run made them and
    # they are empty.
    ki_tmp=
    ki_made_home=
    ki_made_bin=
    trap cleanup EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
    if mkdir -- "$ki_home" 2>/dev/null; then
        ki_made_home=yes
    elif [ ! -d "$ki_home" ]; then
        die "cannot create $ki_home"
    fi
    # Under home, so the final mv is a rename on one file system.
    ki_tmp=$(mktemp -d "$ki_home/.tmp-install.XXXXXX") || die "cannot create a temp dir in $ki_home"

    fetch "$ki_base/latest/download/SHA256SUMS" "$ki_tmp/SHA256SUMS" 1048576
    # The file as downloaded is verified, before any line of it is read.
    if command -v minisign >/dev/null 2>&1; then
        ki_sig=$ki_tmp/SHA256SUMS.minisig
        # In a subshell: fetch exits on failure, and in a local tree no signature is allowed.
        if (fetch "$ki_base/latest/download/SHA256SUMS.minisig" "$ki_sig" 4096) 2>/dev/null; then
            # -H: only the prehashed form minisign makes by default, as kulman takes it.
            minisign -VHm "$ki_tmp/SHA256SUMS" -x "$ki_sig" -P "$ki_key" >/dev/null 2>&1 ||
                die "SHA256SUMS: signature does not verify"
            printf 'install.sh: signature checked\n' >&2
        elif [ -e "$ki_sig" ]; then
            # A file was left past the limit (a local tree, or a server that sends no length):
            # there, but no signature. curl makes no file for one that is absent, nor for one
            # whose announced length is past the limit, so those read as missing below.
            die "SHA256SUMS: signature does not verify"
        elif [ -n "$ki_https" ]; then
            die "SHA256SUMS: signature is missing"
        else
            printf 'install.sh: signature not checked: local release tree\n' >&2
        fi
    else
        printf '%s %s\n' 'install.sh: signature not checked (minisign not found); kulman checks it' \
            'on every install and update' >&2
    fi
    # Without a carriage return at the end of each line, so a CRLF file reads the same.
    ki_cr=$(printf '\r')
    ki_sums=$ki_tmp/SHA256SUMS.lf
    sed "s/$ki_cr\$//" "$ki_tmp/SHA256SUMS" >"$ki_sums"
    ki_name='kulman-v[0-9]+\.[0-9]+\.[0-9]+-'$ki_target'\.tar\.gz'
    ki_count=$(grep -Ec "[ *]$ki_name\$" "$ki_sums" || :)
    [ "$ki_count" = 1 ] ||
        die "SHA256SUMS: $ki_count lines name a kulman archive for $ki_target; expected 1"
    ki_line=$(grep -E "[ *]$ki_name\$" "$ki_sums")
    printf '%s\n' "$ki_line" | grep -Eq "^[0-9a-fA-F]{64} [ *]$ki_name\$" ||
        die "SHA256SUMS: bad line for kulman on $ki_target"
    ki_want=$(printf '%s\n' "${ki_line%% *}" | tr 'A-F' 'a-f')
    ki_file=${ki_line##*[ *]}
    ki_version=${ki_file#kulman-v}
    ki_version=${ki_version%%-*}
    case $ki_version in
        '' | *[!0-9.]*) die "SHA256SUMS: bad version in $ki_file" ;;
    esac

    ki_archive=$ki_tmp/kulman.tar.gz
    fetch "$ki_base/download/v$ki_version/$ki_file" "$ki_archive" 268435456
    # From stdin: given a path holding `\` or a newline, both tools put a `\` before the hash.
    if [ "$ki_sha" = sha256sum ]; then
        ki_got=$(sha256sum <"$ki_archive")
    else
        ki_got=$(shasum -a 256 <"$ki_archive")
    fi
    [ "${ki_got%% *}" = "$ki_want" ] || die "$ki_file: checksum mismatch"

    ki_unpack=$ki_tmp/unpack
    mkdir -- "$ki_unpack"
    tar -xzf "$ki_archive" -C "$ki_unpack" --no-same-owner kulman ||
        die "$ki_file: cannot unpack kulman"
    if [ ! -f "$ki_unpack/kulman" ] || [ -L "$ki_unpack/kulman" ]; then
        die "$ki_file: archive holds no regular file kulman"
    fi
    chmod 755 "$ki_unpack/kulman"
    if [ ! -d "$ki_bin" ]; then
        mkdir -- "$ki_bin"
        ki_made_bin=yes
    fi
    mv -f -- "$ki_unpack/kulman" "$ki_dest"

    printf 'kulman installed in %s\n' "$ki_bin"
    if [ -e "$ki_home/state.json" ]; then
        printf 'kulman replaced; run "%s/kulman update" to bring the modules to the same version\n' \
            "$ki_bin"
    else
        printf 'next: %s/kulman install\n' "$ki_bin"
    fi
}

die() {
    printf 'install.sh: %s\n' "$1" >&2
    exit 1
}

# fetch URL OUT MAX: downloads URL into OUT, failing past MAX bytes.
fetch() {
    if [ -n "$ki_https" ]; then
        # --proto-redir: a redirect to plain http is refused.
        curl --proto '=https' --tlsv1.2 --proto-redir '=https' --fail --silent --show-error \
            --location --connect-timeout 15 --speed-limit 1 --speed-time 60 \
            --max-filesize "$3" --output "$2" "$1" || die "cannot download $1"
    else
        curl --fail --silent --show-error --max-filesize "$3" --output "$2" "$1" ||
            die "cannot download $1"
    fi
    # --max-filesize cannot stop a download whose length is not sent in advance.
    ki_size=$(wc -c <"$2" | tr -d ' ')
    [ "$ki_size" -le "$3" ] || die "$1: larger than $(($3 / 1048576)) MiB"
}

cleanup() {
    if [ -n "$ki_tmp" ]; then
        rm -rf -- "$ki_tmp"
    fi
    if [ -n "$ki_made_bin" ]; then
        rmdir -- "$ki_bin" 2>/dev/null || :
    fi
    if [ -n "$ki_made_home" ]; then
        rmdir -- "$ki_home" 2>/dev/null || :
    fi
}

main "$@"
