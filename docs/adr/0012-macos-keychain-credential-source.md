# Source Claude credentials from a platform credential source (file or macOS Keychain)

On Linux and Windows, Claude Code stores its OAuth credentials in
`~/.claude/.credentials.json`. On macOS it stores them in the login Keychain as
a generic-password item named `Claude Code-credentials` and leaves no
credentials file. Capsule's per-run credential isolation (ADR-0001) and
resume-retry (ADR-0002) both assumed the file, so `capsule run` could not
authenticate the container on a stock macOS install.

We introduce a `CredentialsSource` abstraction that hides the file-vs-Keychain
difference behind one small interface:

- `detect(claude_dir) -> Self` — picks the source. An existing
  `.credentials.json` always wins (every platform); only an absent file on
  macOS falls back to the Keychain. A macOS user who still has a file keeps the
  file-backed path.
- `read() -> Result<Option<Vec<u8>>>` — the current credentials, or `None` when
  none exist.
- `revision() -> Result<HostRevision>` — an opaque marker of host credential
  state used to detect concurrent host-side rotation (see below).
- `write(&[u8]) -> Result<()>` — persist refreshed credentials back to the host.

Variants are **`cfg`-gated** (`File`, and macOS-only `Keychain`), selected by
compile-time platform dispatch rather than runtime injection. Linux/Windows
compile only the `File` variant.

## Host revision generalizes the mtime check

ADR-0001's write-back is conditional on the host not having rotated its token
during the run. For files that was an mtime comparison. `HostRevision`
generalizes it to whatever uniquely identifies the host's current credential
state:

- `Missing` — no credentials present.
- `Mtime(SystemTime)` — file-backed: the `.credentials.json` mtime (cheap, no
  content read).
- `Bytes(Vec<u8>)` — Keychain-backed: the secret bytes themselves (the Keychain
  exposes no usable timestamp).

`CredentialsGuard` snapshots the revision at construction and, on `Drop`,
re-reads it. The full write-back matrix is preserved across both sources: write
when the container refreshed and the host did not; skip when the host rotated
concurrently (revision changed); no-op when the container did not refresh
(temp-file bytes unchanged).

Resume-retry re-reads the host's current credentials *through the source*
(`reload_from_host`) rather than copying a file path, so it works for both file
and Keychain backings.

## Keychain mechanics

Keychain access shells out to the `security` CLI (no Rust Keychain crate):

- **Read / revision**: `security find-generic-password -s "Claude Code-credentials" -w`
  prints the secret to stdout, prompt-free (the `security` binary owns the
  read). A non-zero exit is treated as "no item".
- **Write**: `security add-generic-password -U -a "" -s "Claude Code-credentials" -w`
  with the secret written **twice over stdin** (the value plus the
  confirmation). `-U` updates the existing item in place, and the empty account
  (`-a ""`) matches Claude's null-account item, so no duplicate is created. The
  secret never appears on the process argument list. JSON is normalized to a
  single line before write because the stdin reader is line-based — raw newlines
  in the credentials only ever appear as JSON formatting whitespace, so a
  compact re-serialization is lossless.

A failed or denied Keychain write surfaces a warning and is skipped; the run
still completes.

## Considered options

**A Rust Keychain crate** (e.g. `security-framework`) — adds a dependency and
links against the Security framework. The `security` CLI is already present on
every Mac, keeps the secret off the arg list via stdin, and was validated
end-to-end on real hardware. Rejected to avoid the dependency.

**Runtime injection of the source** (a boxed trait object chosen at startup) —
unnecessary; the platform is known at compile time, so `cfg`-gated enum variants
are simpler and keep the macOS-only `security` code out of other targets.

**Pre-minting or refreshing OAuth tokens ourselves** — rejected in ADR-0002 and
still out of scope; capsule only reads, isolates, and writes back whatever the
host holds.

## Consequences

- The expiry reader (`token_remaining_minutes` / `host_token_is_expired`) reads
  through the source, so the pre-run lifetime warning and the resume-retry
  decision work on macOS. Public signatures are unchanged (still keyed by
  `claude_dir`).
- On macOS, "no credentials file" no longer means "no credentials" — it means
  "consult the Keychain". Tests that assert the file-missing path are gated to
  non-macOS; the Keychain path has its own `cfg(target_os = "macos")`
  round-trip test against a throwaway, self-cleaning service item (no
  `#[requires_docker]`; gated by `cfg(target_os)`).
- Known caveat: because capsule is not in the Claude-owned item's ACL, the first
  write-back may raise a one-time macOS "allow access" dialog; "Always Allow"
  dismisses it permanently. A denied prompt degrades to no-write-back (warning +
  continue), not a failed run.
