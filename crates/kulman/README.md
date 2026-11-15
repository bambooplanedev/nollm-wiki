**English** · [Українська](README.uk.md)

# kulman

**kulman**, said KOOL-mahn (кульман): a design bureau's drawing board — the front door: installs
the tools, sets up a project.

`kulman` installs, updates and sets up the modules of the kulman set: `calque` (the project wiki for
coding agents) and `plaz` (decision charts). It is the installer, not a module. Release 0.8.0 is
the first to carry it; the [repository README](../../README.md) describes the whole set.

## Getting it

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/kulman/releases/latest/download/install.sh | sh

`install.sh` reads `SHA256SUMS` of the latest release (and checks its signature when `minisign` is
installed, see [Signatures](#signatures)), downloads the `kulman` archive for this machine
(`aarch64-apple-darwin` or `x86_64-unknown-linux-musl`; any other system gets "no build"), checks it
against that file and puts the binary at `~/.kulman-home/bin/kulman`. It needs `curl`, `tar`,
`mktemp` and `sha256sum` or `shasum`; without a checksum tool it stops rather than skip the check.
Besides that binary and the folders it goes into, `install.sh` writes nothing and edits no
shell profile. On any failure it exits non-zero and leaves no temp dir; a first install also leaves
no `kulman` in `bin/` (over an installation the old one stays), and a home directory it created is
removed again if empty. Over an installation (`state.json` present) it ends by saying to run `kulman
update`, which brings the modules to the version of the `kulman` it just put in place. The Linux
build is static and runs on glibc and musl systems alike.

By hand, the same: download `kulman-v<version>-<target>.tar.gz`, `SHA256SUMS` and
`SHA256SUMS.minisig` from the release page, then

    minisign -Vm SHA256SUMS -P RWSDr1EMmW7Mh0fB1YXLI+poqKnCnGO9UTdaKQ8QmEm/t4rE6kY1dvZr
    grep kulman-v<version>-<target>.tar.gz SHA256SUMS | sha256sum -c -   # on macOS: … | shasum -a 256 -c -
    tar -xzf kulman-v<version>-<target>.tar.gz kulman
    mkdir -p ~/.kulman-home/bin && mv kulman ~/.kulman-home/bin/

Adding `~/.kulman-home/bin` to `PATH` is optional, for typing `kulman` in a terminal (agents find
`calque` without it; when `~/.local/bin` is on `PATH`, install links the tools there). Then run:

    ~/.kulman-home/bin/kulman install       # kulman and the release's default modules
    cd your-project
    kulman setup --scope local         # each installed module's own setup, in this project

## Where things go

Everything is in `~/.kulman-home` (`KULMAN_HOME` if set):

- `bin/kulman` and `bin/<module>` for each installed module;
- `manifest.json`, the manifest of the installed release;
- `state.json`, the installed version and modules, written last (one that is not a state file
  kulman wrote stops every command, which names it: fix it, or delete it and run `kulman install`;
  one that cannot be read stops it with the path and the cause);
- `.lock`, held by the running command; an install that fails where nothing is installed removes
  it again if it made it, and the directory too if it made it: a home it found is left as it was.

The parent of that directory must exist; only the directory itself is created. A `bin` that is a
symlink or not a directory is refused. No shell profile is edited.

## Commands

    kulman [status]

With no command, or `status`: the installed version and modules (a module whose binary is missing is
marked `(missing: run kulman update)`), the two lines `<home>/bin is not on PATH; to type kulman in
a terminal, add it:` and `export PATH="<home>/bin:$PATH"` (indented) when it is not (a `PATH` entry
that reaches `bin/` through a symlink counts, and so does a link in a `PATH` folder for each of
kulman and the installed modules, when it is the first of its name on `PATH`), and whether a newer
release exists (`newer release: <X.Y.Z> (kulman update)`, or `up to date`). It only reads: nothing
is created. The release host gets 5 s to connect and 8 s in all; past that, or on any error, the
last line says what could not be reached, and why.

    kulman install [--modules <MODULES>] [--version <X.Y.Z>] [--yes]

Installs kulman and the named modules (comma separated; spaces around a name are ignored), with the
modules they require. Without `--modules` it installs the modules the release marks as default; to
leave one out, name the others. `--version` installs that release instead of the latest. Run again
with other modules, it adds them at the installed version, and puts back any listed binary missing
from `bin/`; an installed `bin/kulman` is then left as it is (`kulman update` always replaces it). A
file at `bin/<module>` that `state.json` does not list is never taken as installed: it is replaced
by the downloaded, checked module. When a newer release exists, it still installs at the installed
version and says so (`newer release X.Y.Z: kulman update`); when the latest release cannot be read,
it says that instead (`could not check for a newer release (<base>: <reason>)`). The installed
version is read once `.lock` is held. If the installed version differs from `--version`, or is newer
than the latest release, it changes nothing and exits 1: an older one gets the advice to run `kulman
update`; a newer one, to run `kulman remove`, then `kulman install --version <X.Y.Z>`. When
`$HOME/.local/bin` is a directory on `PATH`, install and update link each installed binary there to
`bin/<name>` (`kulman: linked <names> into <dir>`); anything there that is not a link into `bin/` is
left as it is (`kulman: left <path> as it is (not a kulman link)`), and `kulman remove` deletes only
its own links. A successful install ends with the next steps: the same two lines for `bin/` if it is
not on `PATH`, then `kulman setup` in a project (named by its full path in `bin/` while neither
`bin/` nor every binary is on `PATH`, as above). With nothing to do it writes nothing and says
`already installed: <names> (<version>)` instead (and, as above, a newer release).

    kulman update [--yes]

Updates kulman and every installed module to the latest release, all or none. A release older
than the installed one is not installed: nothing changes. At the installed version with every file
in place it says `up to date` and does nothing. Each module it moves is named with both
versions (`updated <name> <old> -> <new>`), a module the new release requires that was not
installed is named `installed <name> <version>`, and the last line, when the version changed, is
`restart running agent sessions; they keep the old binaries until then`. At the same version, files
put back are named `repaired <name> <version>`, with no restart line.

    kulman remove [--modules <MODULES>] [--yes]

Removes the named modules; without `--modules`, every module, kulman itself, its manifest, state and
lock, then `bin/` and the home directory if they are empty. Only what `state.json` lists is deleted;
any other file stays (with no `state.json`, nothing at all is, and a `bin/kulman` that `install.sh`
put there is named for you to delete). Every path is checked before the first is deleted: a
directory in place of a binary, or anything but a regular file in place of `manifest.json` or
`state.json`, stops it with nothing deleted (exit 1), as does a module that another installed module
requires (`kulman: cannot remove <a>: <b> requires it`; name both to remove both). `kulman` is not a
module: `--modules kulman` is a usage error. At a terminal it asks first (default: no); without one
it needs `--yes`, and otherwise changes nothing and exits 2. Removing everything while calque is
installed also says that projects set up with kulman still name `calque` in their agent configs and
need `kulman setup --scope repo --remove` (or `--scope local`) first: before the question at a
terminal, as the last line otherwise.

    kulman setup [--modules <MODULES>] [--memory | --no-memory] [--agent <LIST>]
                 [--scope repo|local] [--no-hook] [--remove] [--yes]

Runs each chosen module's own `setup` (`~/.kulman-home/bin/<module> setup …`) in the current
directory, in the release's order: every chosen module runs, whatever the earlier ones returned.
Without `--modules` every installed module is set up. `--scope` goes to every module; `--agent`,
`--no-hook` and `--memory`/`--no-memory` are calque's and are a usage error when calque is not
chosen. Agent memory is on unless `--no-memory` is given (or the release turns it off). `--remove`
undoes each module's setup. Before the first module runs, every chosen module's binary must be in
`bin/` and executable; with no module installed there is nothing to set up (exit 1). The modules
write into the project's `.kulman/`, a folder of its own: setup refuses (exit 1) to run where that
folder is kulman's home (`KULMAN_HOME` set to `<project>/.kulman`). When `bin/` is not on `PATH`,
the modules run with it put first on their own `PATH` (kulman's own environment and your shell are
not changed; a `bin/` path holding a `:` cannot be on a `PATH`, so theirs is then left as it is).
The agent configs calque writes start it from the kulman home, so they need nothing on `PATH`.

## Asking and not asking

People and agents get the same commands. A command asks only when stdin and stdout are both
terminals and `--yes` is absent; otherwise it asks nothing and the defaults apply (`install`: the
release's defaults; `setup`: every installed module, memory on). A value with no default must then
be given as a flag: `kulman setup` without a terminal needs `--scope`. When kulman does not ask,
the modules it runs get no input, so they ask nothing either. `remove`, whose question
defaults to no, does nothing without a terminal unless `--yes` is given. A confirmation is asked
once the command holds `.lock` and has read the state, so what it shows is what it does. Names
typed at a prompt match in any case.

## Exit codes

- `0` — done, or nothing to do.
- `1` — failed; the message says why (a download, what and from where). A failed `install` or
  `update` that changed nothing ends with the line `nothing changed`.
- `2` — usage error (an unknown flag, an empty or malformed name in `--modules` or one the release
  does not have, a missing `--scope`, a bad `KULMAN_RELEASE_URL`, `remove` without a terminal and
  without `--yes`).

Each module whose setup fails gets the line `kulman: <module> setup failed (exit N)` (or
`(signal N)`); the others still run. Then `kulman setup` prints `kulman: setup finished with
errors: <modules>` and exits with the status of the first module that failed (1 if it was killed
by a signal).

## Environment

- `KULMAN_HOME` — the home directory (default `$HOME/.kulman-home`; empty counts as unset; a path containing `"` or `$` is not
  supported by the agent launch line calque writes).
- `KULMAN_RELEASE_URL` — where releases are (default
  `https://github.com/bambooplanedev/kulman/releases`): `https://…` or `file://…`, nothing else.

## One run at a time, and interruptions

A command that changes `~/.kulman-home` holds `.lock`, questions included; a second one waits for
it and says so once on stderr: `kulman: waiting for another kulman run (<home>/.lock)`.
Downloads are checked and unpacked into a temp dir under home (`.tmp-<pid>`) before anything in
`bin/` changes; then the binaries are renamed into `bin/` (kulman last), then `manifest.json`, then
`state.json`. `install.sh` takes no lock, and its temp dir (`.tmp-install.…`) is one of the `.tmp-*`
entries that `kulman install`, `update` and `remove` delete in a home with a `state.json`: do not
start one of them while `install.sh` runs, or `install.sh` fails.

- Interrupted before that point, an install or update leaves only its temp dir, which the next run
  that finds `state.json` deletes (a home without one is never cleaned).
- Interrupted while renaming, some binaries are at the new version and `state.json` still names the
  old one. `kulman update` installs the whole release again.
- A binary that `state.json` lists but `bin/` lacks (a `remove` interrupted, or a file deleted by
  hand) is put back by `kulman update` or `kulman install`, and a missing `manifest.json` by
  `kulman update`; `kulman setup` says so instead of running.

## A release

A release holds, under `<base>/download/v<version>/`:

- `manifest.json` — the release's only source of truth for kulman;
- `<module>-v<version>-<target>.tar.gz` and `kulman-v<version>-<target>.tar.gz`, each holding one
  file named after its binary;
- `SHA256SUMS` — `<sha256>  <file>` lines, for `install.sh` and for people;
- `install.sh`;
- `manifest.json.minisig` and `SHA256SUMS.minisig` — the signatures of those two files (see
  [Signatures](#signatures)).

`<base>/latest/download/` serves the same files of the latest release. kulman reads the manifest
from there; from `download/v<version>/` with `--version`; and from `download/v<installed>/` for an
`install` over an installed version when the latest release is newer or cannot be read. It reads
every archive from `download/v<version>/`. `install.sh` reads `SHA256SUMS` from
`latest/download/`.

`manifest.json`:

```json
{
  "version": "0.8.1",
  "modules": [
    {
      "name": "calque",
      "summary": "the project wiki for coding agents",
      "default": true,
      "requires": [],
      "options": [{ "name": "memory", "default": true }],
      "archives": {
        "aarch64-apple-darwin": { "file": "calque-v0.8.1-aarch64-apple-darwin.tar.gz", "sha256": "…" }
      }
    }
  ],
  "kulman": {
    "archives": {
      "aarch64-apple-darwin": { "file": "kulman-v0.8.1-aarch64-apple-darwin.tar.gz", "sha256": "…" }
    }
  }
}
```

Unknown fields are ignored. Each archive is checked against its `sha256` before it is unpacked.

### Signatures

A published release holds 11 files: the 6 archives, `manifest.json`, `SHA256SUMS`, `install.sh`
and two detached [minisign](https://jedisct1.github.io/minisign/) signatures,
`manifest.json.minisig` and `SHA256SUMS.minisig`. Only `manifest.json` and `SHA256SUMS` are
signed; the archives are covered by the SHA-256 sums inside them. The release key is
[`release/kulman.pub`](../../release/kulman.pub) in this repository:

```
untrusted comment: minisign public key 87CC6E990C51AF83
RWSDr1EMmW7Mh0fB1YXLI+poqKnCnGO9UTdaKQ8QmEm/t4rE6kY1dvZr
```

`kulman` has the key compiled in; no flag or variable changes it. With every `manifest.json` it
reads it fetches `manifest.json.minisig` from beside it and checks it before anything in the
manifest is used. Only the form minisign makes by default (prehashed) is taken, not the legacy one
(`minisign -S -l`). Over `https://` the signature is required, and a manifest without a good one is
not used: `install` and `update` stop with `kulman: manifest.json: signature is missing` (the
release host answers 404), `kulman: manifest.json: signature could not be fetched: <cause>` (any
other failure: a timeout, TLS, a server error; in a `file://` tree, a `.minisig` that cannot be
read) or `kulman: manifest.json: signature does not verify`, then `nothing changed`, exit 1. Where a
command only looks at the latest release for a newer one (`kulman status`, and `install` over an
installed version), its `could not check for a newer release (…)` line gives the reason instead. In
a `file://` tree a signature that is there must verify too; one that is absent is allowed, and
`kulman` says `signature not checked: local release tree` once, on stderr: it is a notice, not a
result.

`install.sh` carries the same key. With `minisign` on `PATH` it downloads `SHA256SUMS.minisig` and
checks `SHA256SUMS` with it (`minisign -VHm`: like `kulman`, only the prehashed form) before it
reads a line of it; it stops (exit 1) with `install.sh: SHA256SUMS: signature is missing` (over
`https://`, nothing was downloaded: no signature, no answer, or a server that announces one past 4
KiB) or `install.sh: SHA256SUMS: signature does not verify` (also for a signature past 4 KiB that
was partly downloaded, as from a local tree or a server that sends no length). The check needs a
minisign that knows `-H`; 0.11 and 0.12 do, and with one that does not, the check fails and
`install.sh` stops. In a `file://` tree an absent signature is allowed, with the line `install.sh:
signature not checked: local release tree`. Without `minisign` it goes on by SHA-256 alone and says
so: `install.sh: signature not checked (minisign not found); kulman checks it on every install and
update`. Both lines go to stderr, so a script that captures the output does not get them.

By hand, with each file and its `.minisig` in one folder:

    minisign -Vm manifest.json -P RWSDr1EMmW7Mh0fB1YXLI+poqKnCnGO9UTdaKQ8QmEm/t4rE6kY1dvZr
    minisign -Vm SHA256SUMS -P RWSDr1EMmW7Mh0fB1YXLI+poqKnCnGO9UTdaKQ8QmEm/t4rE6kY1dvZr

Signatures came before any release of `kulman` was published: every published `kulman` checks
them, so every release must be signed, or `install` and `update` refuse it.

Signing a release: a pushed `v*` tag makes a draft release with the 9 files (the release
workflow); nothing in GitHub holds the secret key. The owner then runs, in a terminal, in a
checkout that has the tag:

    # with --key <secret key file> if the key is not minisign's default
    sh scripts/release-sign.sh <version>

It checks the draft (this repository's: under the releases address `kulman` uses; it prints the
repository and the release's address before signing; exactly the 9 files; each archive's SHA-256 as
`SHA256SUMS` and `manifest.json` state it; the version; `install.sh` as at the tag), signs the two
files (minisign asks for the key's password, once per file), verifies both with `release/kulman.pub`
(a signature by any other key stops it, with nothing uploaded) and uploads the two `.minisig` files
to the draft; `--replace` signs a draft that has them again. It does not publish: it ends with the
command that does, `gh release edit v<version> --draft=false`. Once published, the release is the
latest one, the one `install.sh` and `kulman update` take.

To try a signed draft before publishing it, download it into `$dl` (`gh release download v$v --dir
"$dl"`, with `v` the version) and lay it out as a release tree at `$out`:

    mkdir -p "$out/download/v$v" "$out/latest/download"
    cp "$dl"/* "$out/download/v$v/"
    cp "$dl"/SHA256SUMS "$dl"/SHA256SUMS.minisig "$dl"/manifest.json "$dl"/manifest.json.minisig \
        "$dl"/install.sh "$out/latest/download/"

Then `KULMAN_RELEASE_URL=file://$out` makes `install.sh` and `kulman` install the candidate, with
its signatures checked. Do not run `release-assemble.sh` on the downloaded files: it rewrites the
two signed files.

Limits:

- One key, no rotation yet. A new key can reach users only in a release signed by the old one, so
  losing the secret key (or its password) leaves no way to make a release `kulman` accepts: keep a
  backup of it.
- A signature shows that the files are the owner's, not that they are the newest. Whoever controls
  the release host could serve an older signed release as the latest: `kulman update` refuses to go
  back to an older version, but a fresh install would take it.
- `install.sh` itself comes over HTTPS and is not signed: whoever controls the place it is
  downloaded from controls that first step, with or without `minisign`. The signatures protect
  what `install.sh` and `kulman` download after it. To trust no download host, check by hand (see
  [Getting it](#getting-it)).
