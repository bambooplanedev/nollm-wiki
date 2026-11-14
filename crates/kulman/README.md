# kulman

`kulman` installs, updates and sets up the modules of the kulman set: `kalka` (the project wiki for
coding agents) and `plaz` (decision charts). It is the installer, not a module.

Available from the first release of the set (0.7.0); no release with `kulman` exists yet. Until
then, build the tools from source (see the [repository README](../../README.md)).

## Getting it

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/kulman/releases/latest/download/install.sh | sh

`install.sh` reads `SHA256SUMS` of the latest release, downloads the `kulman` archive for this
machine (`aarch64-apple-darwin`, `x86_64-apple-darwin` or `x86_64-unknown-linux-gnu`), checks it
against that file and puts the binary at `~/.kulman/bin/kulman`. It needs `curl`, `tar` and
`sha256sum` or `shasum`; without a checksum tool it stops rather than skip the check. It writes
nothing else and edits no shell profile. On any failure it exits non-zero and leaves no `kulman`
in `bin/` and no temp dir; a home directory it created is removed again if empty. Over an
installation (`state.json` present) it ends by saying to run `kulman update`, which brings the
modules to the version of the `kulman` it just put in place.

By hand, the same: download `kulman-v<version>-<target>.tar.gz` and `SHA256SUMS` from the release
page, then

    sha256sum -c --ignore-missing SHA256SUMS      # on macOS: shasum -a 256 -c --ignore-missing SHA256SUMS
    tar -xzf kulman-v<version>-<target>.tar.gz kulman
    mkdir -p ~/.kulman/bin && mv kulman ~/.kulman/bin/

Then add `~/.kulman/bin` to `PATH` yourself (agents start `kalka` by its name) and run:

    ~/.kulman/bin/kulman install       # kulman and the release's default modules
    cd your-project
    kulman setup --scope local         # each installed module's own setup, in this project

## Where things go

Everything is in `~/.kulman` (`KULMAN_HOME` if set):

- `bin/kulman` and `bin/<module>` for each installed module;
- `manifest.json`, the manifest of the installed release;
- `state.json`, the installed version and modules, written last (one kulman cannot read stops every
  command, which names it: fix it, or delete it and run `kulman install`);
- `.lock`, held by the running command; an install that fails where nothing is installed removes
  it again, and the directory too if it made it.

The parent of that directory must exist; only the directory itself is created. A `bin` that is a
symlink or not a directory is refused. No shell profile is edited.

## Commands

    kulman

With no command: the installed version and modules (a module whose binary is missing is marked
`(missing: run kulman update)`), and whether a newer release exists. It only reads: nothing is
created. The release host gets 5 s to connect and 8 s in all; past that, or on any error, the
last line says what could not be reached, and why.

    kulman install [--modules <MODULES>] [--version <X.Y.Z>] [--yes]

Installs kulman and the named modules (comma separated; spaces around a name are ignored), with
the modules they require. Without `--modules` it installs the modules the release marks as
default; to leave one out, name the others.
`--version` installs that release instead of the latest. Run again with other modules, it adds them
at the installed version, and puts back any listed binary missing from `bin/`; an installed
`bin/kulman` is then left as it is (`kulman update` always replaces it). When a newer release
exists, it still installs at the installed version and says so (`newer release X.Y.Z: kulman
update`). If the installed version differs from `--version`, or is newer than the latest release,
it changes nothing and exits 1: an older one gets the advice to run `kulman update`; a newer one,
to run `kulman remove`, then `kulman install --version <X.Y.Z>`. A successful install ends with
the next steps: adding `bin/` to `PATH` if it is not there, then `kulman setup` in a project.

    kulman update [--yes]

Updates kulman and every installed module to the latest release, all or none. A release older
than the installed one is not installed: nothing changes.

    kulman remove [--modules <MODULES>] [--yes]

Removes the named modules; without `--modules`, every module, kulman itself, its manifest, state
and lock, then `bin/` and the home directory if they are empty. Only what `state.json` lists is
deleted; any other file stays (with no `state.json`, nothing at all is, and a `bin/kulman` that
`install.sh` put there is named for you to delete). `kulman` is not a module: `--modules kulman`
is a usage error.
At a terminal it asks first (default: no); without one it needs `--yes`, and otherwise changes
nothing and exits 2.

    kulman setup [--modules <MODULES>] [--memory | --no-memory] [--agent <LIST>]
                 [--scope repo|local] [--no-hook] [--remove] [--yes]

Runs each chosen module's own `setup` (`~/.kulman/bin/<module> setup …`) in the current directory,
in the release's order, and stops at the first that fails. Without `--modules` every installed
module is set up. `--scope` goes to every module; `--agent`, `--no-hook` and `--memory`/
`--no-memory` are kalka's and are a usage error when kalka is not chosen. Agent memory is on unless
`--no-memory` is given (or the release turns it off). `--remove` undoes each module's setup.
Before the first module runs, every chosen module's binary must be in `bin/` and executable; with
no module installed there is nothing to set up (exit 1).

## Asking and not asking

People and agents get the same commands. A command asks only when stdin and stdout are both
terminals and `--yes` is absent; otherwise it asks nothing and the defaults apply (`install`: the
release's defaults; `setup`: every installed module, memory on). A value with no default must then
be given as a flag: `kulman setup` without a terminal needs `--scope`. `remove`, whose question
defaults to no, does nothing without a terminal unless `--yes` is given. A confirmation is asked
once the command holds `.lock` and has read the state, so what it shows is what it does. Names
typed at a prompt match in any case.

## Exit codes

- `0` — done, or nothing to do.
- `1` — failed; the message says why (a download, what and from where). A failed `install` or
  `update` that changed nothing ends with the line `nothing changed`.
- `2` — usage error (an unknown flag, an empty or malformed name in `--modules` or one the release
  does not have, a missing `--scope`, a bad `KULMAN_RELEASE_URL`, `remove` without a terminal and without `--yes`).

`kulman setup` exits with a module's own status when that module's setup fails (1 if it was
killed by a signal), after the line `kulman: <module> setup failed (exit N)` (or `(signal N)`).

## Environment

- `KULMAN_HOME` — the home directory (default `$HOME/.kulman`; empty counts as unset).
- `KULMAN_RELEASE_URL` — where releases are (default
  `https://github.com/bambooplanedev/kulman/releases`): `https://…` or `file://…`, nothing else.

## One run at a time, and interruptions

A command that changes `~/.kulman` holds `.lock`; a second one waits for it. Downloads are checked
and unpacked into a temp dir under home (`.tmp-<pid>`) before anything in `bin/` changes; then the
binaries are renamed into `bin/` (kulman last), then `manifest.json`, then `state.json`.

- Interrupted before that point, an install or update leaves only its temp dir, which the next run
  deletes.
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
- `install.sh`.

`<base>/latest/download/` serves the same files of the latest release. kulman reads the manifest
from there (or from `download/v<version>/` with `--version`, and for `install` over an installed
version the latest release is not) and every archive from
`download/v<version>/`; `install.sh` reads `SHA256SUMS` from there.

`manifest.json`:

```json
{
  "version": "0.7.0",
  "modules": [
    {
      "name": "kalka",
      "summary": "the project wiki for coding agents",
      "default": true,
      "requires": [],
      "options": [{ "name": "memory", "default": true }],
      "archives": {
        "aarch64-apple-darwin": { "file": "kalka-v0.7.0-aarch64-apple-darwin.tar.gz", "sha256": "…" }
      }
    }
  ],
  "kulman": {
    "archives": {
      "aarch64-apple-darwin": { "file": "kulman-v0.7.0-aarch64-apple-darwin.tar.gz", "sha256": "…" }
    }
  }
}
```

Unknown fields are ignored. Each archive is checked against its `sha256` before it is unpacked.
