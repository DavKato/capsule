# Isolate `.credentials.json` per run to prevent concurrent session token races

When the host Claude Code session and a container session both share the same
`~/.claude/.credentials.json`, concurrent token rotation causes one side to
present a token the server has already invalidated — manifesting as
`authentication_failed` on the container's second iteration (issue #55).

At run start, capsule copies `.credentials.json` to a `NamedTempFile` and
mounts that copy over the credentials path inside the directory mount, giving
the container an independent credential snapshot for the duration of the run.
A `CredentialsGuard` owns the temp file and implements conditional write-back
in its `Drop` impl, so write-back fires on all exit paths (clean, error, or
signal-driven drop) — not only on clean exit.

Write-back is conditional on two checks:

1. **Host file unchanged** — if the host file's mtime changed since the
   snapshot was taken, the host refreshed its own token during the run.
   Writing back would clobber the host's newer credentials, potentially
   invalidating its refresh token. Write-back is skipped.
2. **Container refreshed** — if the temp file content is identical to the
   original snapshot, the container never refreshed. Write-back is a no-op
   and is skipped.

Only when both conditions hold (host file untouched, container content
changed) does the guard write back, propagating the container's refreshed
token to the host.

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
shared writable mount. Conditional write-back on drop preserves refreshed
tokens for the host without clobbering concurrently refreshed host tokens.
