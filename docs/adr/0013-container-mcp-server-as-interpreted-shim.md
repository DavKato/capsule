# Run the container's MCP server as an interpreted shim, not the mounted host binary

The container needs a `capsule` MCP server so Claude Code can call
`submit_verdict`. Originally capsule bind-mounted its own host binary
(`std::env::current_exe()`) into the container at `/usr/local/bin/capsule` and
pointed `.mcp.json` at `capsule mcp-serve`.

That only works when the host binary is executable in the container — i.e. when
host OS + arch + libc match the container's. It breaks in exactly the case we
now support:

- **macOS host** → the host binary is a Mach-O, which cannot run on the Linux
  container at all (and on Apple Silicon the container is `linux/arm64`). The
  MCP server never starts, `submit_verdict` is never registered, and no verdict
  can be produced.
- **Linux host, mismatched libc** → even an ELF can fail: a binary dynamically
  linked against a newer glibc (e.g. Arch) can hit `GLIBC_x.xx not found` on the
  Ubuntu base. This was masked only while the base image was also Arch.

## The server is a stateless echo

The decisive observation: the container-side MCP server holds no state and has
no back-channel to the orchestrator. capsule reads the verdict by parsing the
`tool_use` block in Claude's `stream-json` output (`stream_parser.rs`), **not**
from the MCP server's reply. The server only needs to (1) register
`submit_verdict` in `tools/list` so Claude can call it, and (2) return any valid
tool result so the turn completes. Its response is otherwise discarded.

So the program that runs in the container needs no capsule logic — and
therefore no capsule binary.

## Decision

Replace the host-binary mount with a tiny **interpreted** stdio JSON-RPC shim
(`src/container_execution/mcp_shim.js`), run by the `node` already present in
the base image. `.mcp.json` invokes `node <shim> <manifest>`. Because it's
source interpreted by a runtime already in the image, one shim runs identically
on `linux/amd64` and `linux/arm64` under any libc, with no host binary, no
cross-compile, no downloaded artifact, and no version-matched packaging.

Rust stays the single source of truth for the protocol. `mcp_server.rs` exposes
`initialize_result()` / `tools_list_result()` (shared with the existing
`capsule mcp-serve`), and `shim_manifest_json()` serializes them into a manifest
that capsule writes to a temp file and mounts. The shim carries no protocol
knowledge of its own: it replays the canned per-method results and validates
`submit_verdict`'s `status` against the enum embedded in the manifest's schema.

## Considered options

**Compile a Rust shim (or ship `capsule`) for the container triple** — a
compiled artifact must match the container's target triple. Producing it means
either cross-compiling on the user's machine (end users have no Rust toolchain),
building inside a Docker stage (needs the source, drags a Rust toolchain and
minutes of build for a discarded echo), or downloading the CI-built
`capsule-<arch>-unknown-linux-gnu` artifact (needs a published release; no
artifact for local/dev builds). Every path pays a real cost to ship a stateless
echo. Justified only if the container side ever needs to share real Rust logic
with capsule — at which point download/bundle the CI artifact.

**Host-side MCP server over HTTP** — the orchestrator hosts `submit_verdict` on
localhost and the container connects via `host.docker.internal`. Keeps Rust as
the sole implementation, but needs an HTTP transport, port management, and an
auth token for a listening socket. Heavier than a stdio shim for no extra
benefit while the server is a stateless echo.

## Consequences

- No capsule binary is mounted into the container; `current_exe()` is no longer
  used for the run. The shim and the per-run manifest are mounted read-only
  alongside `.mcp.json`.
- `capsule mcp-serve` / `run_server` remain as the canonical Rust server (and
  for any direct/stdio use); the shim is derived from the same response
  builders, so the two cannot drift on schema. A unit test pins the manifest
  contents and a `#[requires_docker]` test round-trips the shim under `node`
  inside the `capsule` image (now host-OS-independent — it no longer skips on
  macOS).
- The base image must contain `node` (it already installs `nodejs`). The
  `submit_verdict`-missing error hint now points at a stale image / missing node
  rather than a binary not on PATH.
