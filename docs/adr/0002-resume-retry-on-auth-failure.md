# Resume-retry on credential expiry instead of failing the run

When the host user's Claude Code session refreshes its OAuth token during a
capsule run, Anthropic's server revokes the refresh token that the container's
credential copy holds. If the container's access token then expires (~5-hour
lifetime), the container cannot refresh and the run fails with
`authentication_failed` — even though the host's credentials are perfectly
valid.

We chose a **resume-retry** strategy: on auth failure, capsule re-copies the
host's current credentials into the container mount and re-launches the
container with `claude --resume <session_id>`, recovering the full
conversation context. A pre-run check (`min_token_lifetime_minutes`) optionally
warns the user before starting if the token is near expiry.

## Considered options

**Pre-refresh the token at prepare() time** — capsule calls the Anthropic OAuth
refresh endpoint itself before starting. Eliminates the problem entirely, but
requires hardcoding endpoint details that could change, and revokes the host's
refresh token (requiring an immediate write-back). Rejected: too fragile and
couples capsule to Anthropic's OAuth implementation.

**Refuse to start when the token is near expiry** — simple `expiresAt` check
at `prepare()`. Prevents wasted runs, but doesn't self-heal. Kept as an
optional pre-check (`min_token_lifetime_minutes`), not the primary defense.

**Roll credentials forward between stages (Option A from issue analysis)** —
re-copy host credentials before each stage. Reduces the failure window but
doesn't eliminate it: the access token at copy time may already be near expiry,
and the host may refresh (revoking the copied refresh token) during the stage.

**Background sync thread** — poll the host credentials file and push updates
into the container mount. Handles mid-stage refresh but adds threading
complexity, and it's unknown whether Claude Code re-reads credentials from disk
after a failed refresh attempt.

**Read-only live mount** — mount the host credentials file directly (no copy).
Eliminates staleness but may cause Claude Code to hard-fail on write when it
tries to persist a refreshed token.

## Consequences

- `StreamParser` must capture `session_id` from the init event and surface it
  to the caller.
- `entrypoint.sh` gains a branch: when `CAPSULE_RESUME_SESSION` is set, it
  invokes `claude --resume` instead of piping `prompt.txt`.
- `CredentialsGuard` stays as-is — its copy-and-write-back logic still handles
  the normal case where the container refreshes successfully without host
  interference.
- `last-run.json` gains a `session_id` field. On mid-session exits only, it
  also includes pipeline state (stage index, counters, fail counts), enabling
  a future `capsule resume` command (no arguments — all state from
  `last-run.json`) to restore the executor and continue the pipeline.
