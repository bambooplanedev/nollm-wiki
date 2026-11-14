# techlog

**techlog**, said TEK-log: an aircraft's technical log — the agent's memory of the project.

A library crate of the [kulman set](../../README.md), used by [kalka](../kalka/README.md). It knows
where agent memory lives: the project's own `.agents/memory/` and the folders agents keep outside it
(today Claude Code's per-project `memory/` folder), and for a committed note, who last changed it and
whether it is edited. What kalka does with the notes is in kalka's
[Agent memory](../kalka/README.md#agent-memory).

It has no binary and is not published on crates.io. `cargo test -p techlog` runs its tests.
