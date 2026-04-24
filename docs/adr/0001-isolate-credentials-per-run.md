# Isolate `.credentials.json` per run to prevent concurrent session token races

When the host Claude Code session and a container session both share the same
`~/.claude/.credentials.json`, concurrent token rotation causes one side to
present a token the server has already invalidated — manifesting as
`authentication_failed` on the container's second iteration (issue #55).

At run start, capsule copies `.credentials.json` to a `NamedTempFile` and
mounts that copy over the credentials path inside the directory mount, giving
the container an independent credential snapshot for the duration of the run.
On clean exit, the temp file is written back to `~/.claude/.credentials.json`
so the host retains any tokens the container refreshed during a long run.

## Considered options

**Full read-only mount of `~/.claude`** — prevents all container writes but
breaks Claude Code if it needs to write session state, and loses the project
memory sharing that was deliberately enabled by matching the container workdir
to the host path.

**Full ephemeral copy of `~/.claude`** — fully isolated but discards the
container's session memory contributions and copies large files (history, file
cache) unnecessarily.

**Shadow only `.credentials.json`** (chosen) — surgical: only the file that
races is isolated; project memory, settings, skills, and plugins remain on the
shared writable mount. Write-back on exit preserves refreshed tokens for the
host.
