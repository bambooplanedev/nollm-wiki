# Installing or updating kulman: steps for a coding agent

You run the commands below yourself and ask the user only for approvals, choices and restarts.

## 1. Find out what is installed

Run `kulman --version`. If the shell cannot find it, try `~/.kulman-home/bin/kulman --version`.

**Missing.** Tell the user you will run the release installer: it downloads the release, checks
its SHA-256 and, when `minisign` is installed, its signature, and installs into
`~/.kulman-home` in their home folder, with no sudo. After they agree, run:

    curl --proto '=https' --tlsv1.2 -sSfL https://github.com/bambooplanedev/kulman/releases/latest/download/install.sh | sh
    ~/.kulman-home/bin/kulman install --yes

The installer prints nothing about the signature when it checked it. When it did not, it
prints `install.sh: signature not checked (minisign not found); kulman checks it on every install
and update`; tell the user that. Set `Installed:` to the version `kulman --version` now prints.

**Installed.** Run `kulman` (no arguments). The first line is `kulman <installed> in <home>`.
The last line is `newer release: <newest> (kulman update)`, `up to date`, or `could not check for a
newer release (...)`; in that case show the line to the user and do not update. If a newer
release exists, tell the user both versions and ask whether to update. On yes run
`kulman update --yes`; it prints `updated <module> <old> -> <new>` per module and then
`restart running agent sessions; they keep the old binaries until then`.

## 2. Set up the project

In the project root run `kulman setup --yes --scope repo --agent <this agent>`, where the agent is
one of `claude`, `codex`, `cursor`, `pi`. Ask the user first whether the files go in the repo
(`--scope repo`, committed for the team) or stay local (`--scope local`). `kulman update` does not
set projects up again, so run this after an update too; it is safe to run again and reports
`already set up: ...` for what is there.

## 3. Settings

Keep the user's existing settings. Edit only this agent's own configuration.

## 4. Hints

Show the user every hint the commands printed and apply each only after they say yes:

- `<bin> is not on PATH; add it:` followed by an `export PATH=...` line: the user adds it to
  their shell profile themselves. Show the line; do not edit the profile.
- `skipped AGENTS.md calque navigation steps: an AGENTS.md beside CLAUDE.md hides CLAUDE.md from
  pi; ...`: the rename it names, if the user wants it.
- `Codex loads the server only in a project it trusts: ...`: the trust it names.
- `Cursor: ... cursor-agent mcp enable calque`: run it.

## 5. Restart

Tell the user to restart this agent session so it loads the new tools. Do not wait for it.

## 6. Check

After the restart, call the `search` tool of the `calque` server (in Claude Code
`mcp__calque__search`) with any word from the project. If the tool is not there, report `pending`.

## Final report

One line each:

    Installed: <version, or "already there">
    Updated: <old -> new, or "no">
    Agents: <the agents set up>
    Verified: <the search result, or "pending">
    Next step for me: <what the user still has to do>

If a step fails, stop and show the exact output.

## Do not

- Use sudo.
- Edit shell startup files.
- Change files outside the project and this agent's own configuration without asking.
- Skip the signature or checksum check.
