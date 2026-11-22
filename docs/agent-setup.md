# Installing or updating nollm-wiki: steps for a coding agent

You run the commands below yourself and ask the user only for approvals, choices and restarts.
Wherever this guide says `nollm-wiki-install`, run `~/.nollm-wiki-home/bin/nollm-wiki-install` if the shell does not find
`nollm-wiki-install` (a fresh install is not on PATH). If this guide and the installed tool differ (a flag, an
output line), follow the tool (`nollm-wiki-install --help`) and tell the user.

## 1. Find out what is installed

Run `nollm-wiki-install --version`. If the shell cannot find it, try `~/.nollm-wiki-home/bin/nollm-wiki-install --version`.

**Missing.** Tell the user you will run the release installer: it downloads the release, checks
its SHA-256 and, when `minisign` is installed, its signature, and installs the `nollm-wiki-install` installer
into `~/.nollm-wiki-home` in their home folder, with no sudo. Then `nollm-wiki-install install --yes` installs the
modules `wiki` (project wiki, search, MCP server) and `plaz` (decision charts). After they
agree, run:

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/nollm-wiki/releases/latest/download/install.sh | sh
    ~/.nollm-wiki-home/bin/nollm-wiki-install install --yes

The installer prints `install.sh: signature checked` when it checked the signature. When it did
not, it prints `install.sh: signature not checked (minisign not found); nollm-wiki-install checks it on every
install and update` (or `... not checked: local release tree`); tell the user that. Set `Installed:` to the version `nollm-wiki-install --version` now prints.

**Installed.** Run `nollm-wiki-install` (no arguments). If it prints `nothing installed in <home>; run nollm-wiki-install
install`, nollm-wiki-install is there but no modules are: say you will install `wiki` and `plaz`, and after
the user agrees run `nollm-wiki-install install --yes`. Otherwise the first line is `nollm-wiki-install <installed> in
<home>`. The last line is `newer release: <newest> (nollm-wiki-install update)`, `up to date`, or `could not
check for a newer release (...)`; in that case show the line to the user and do not update. If a
newer release exists, tell the user both versions and ask whether to update. On yes run
`nollm-wiki-install update --yes`; it prints `updated <module> <old> -> <new>` per module and then
`restart running agent sessions; they keep the old binaries until then`.

## 2. Set up the project

If this agent is none of `claude`, `codex`, `cursor`, `pi`, tell the user that nollm-wiki has no setup
for it, and stop here: the tools are installed, the project is not set up.

Ask the user first. Setup writes in the project: the `.nollm-wiki/` folder with the compiled wiki and
decision charts; `.agents/memory`, the agent memory folder; this agent's files, `.mcp.json` and
`.claude/settings.json` for Claude Code, `.cursor/mcp.json` for Cursor, `.codex/config.toml` and a
navigation block in `AGENTS.md` for Codex, `.pi/` and `.agents/skills/` for pi; and lines that
keep `.nollm-wiki/` out of git. Two scopes: `repo` writes the files for the whole team to commit and
puts the ignore line in `.gitignore`; `local` is for this checkout only, with the ignore lines in
`.git/info/exclude` (for Claude Code the server goes in Claude's own local config). The prompt hook
(`.claude/settings.local.json` or `.claude/settings.json`, `.pi/extensions/wiki-hint/`) is written
only with `--hook`, and a run without it removes one an earlier setup wrote.

After they answer, in the project root run:

    nollm-wiki-install setup --yes --scope <repo|local> --agent <agent>

Run it after an update too: `nollm-wiki-install update` does not set projects up again. It is safe to repeat.

Do not edit files yourself; `setup` writes what it needs. Keep the user's existing settings.

## 3. Hints

Show the user every hint the commands printed and apply each only after they say yes:

- `<bin> is not on PATH; to type nollm-wiki-install in a terminal, add it:` followed by an indented
  `export PATH=...` line: optional, a convenience for typing `nollm-wiki-install` and the modules in a
  terminal. The agents find `wiki` without PATH. If the user wants it, they add the line to their
  shell profile themselves; show it, do not edit the profile. It is not a "Next step for me"
  item unless the user asks for it.
- `nollm-wiki-install: linked <names> into <dir>`: nothing to do.
- `nollm-wiki-install: left <path> as it is (not a nollm-wiki-install link)`: tell the user; do not delete or replace that
  file.
- `warning: <file> is ignored by git; teammates will not get it`: tell the user; do not change
  their ignore rules unless they say yes.
- `warning: the agents find wiki through NOLLM_WIKI_HOME; set it in their environment`: tell the
  user; nothing is edited for them.
- `warning: skipped AGENTS.md wiki navigation steps: an AGENTS.md beside CLAUDE.md hides
  CLAUDE.md from pi; ...`: the rename it names, if the user wants it. The server is set up and
  setup exits 0.
- `warning: skipped .pi/APPEND_SYSTEM.md wiki navigation steps: a project .pi/APPEND_SYSTEM.md
  would replace pi's global APPEND_SYSTEM.md ...`: the copy it names, if the user wants it. The
  server and skill are set up and setup exits 0.
- `warning: <dir>/AGENTS.md holds an older wiki navigation block, loaded in every pi project; ...`:
  tell the user; removing it is theirs to do.
- `Codex loads the server only in a project it trusts: ...`: the trust it names.
- `Cursor: ... cursor-agent mcp enable wiki`: run it.

## 4. Report, then restart

The session after a restart does not have this guide: give the final report first, then tell the
user to restart this agent session so it loads the new tools.

## Final report

One line each:

    Installed: <version, or "already there">
    Updated: <old -> new, or "no">
    Agents: <the agents set up>
    Verified: pending — after the restart ask me to search the project with wiki
    Signature: <the installer's own line, `install.sh: signature checked` or its "not checked"
      line; "no installer run" if it did not run; never inferred>
    Next step for me: <restart the agent; plus any hint above the user asked to act on>

If a step fails, stop and show the exact output.

## 5. Check, if the user asks after the restart

Call the `search` tool of the `wiki` server (in Claude Code `mcp__wiki__search`) with any word
from the project. If the tool is not there, say so.

## Do not

- Use sudo, or edit shell startup files.
- Change files outside the project and this agent's own configuration without asking.
- Skip the signature or checksum check.
