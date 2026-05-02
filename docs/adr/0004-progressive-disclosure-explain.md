# Agent-facing docs use progressive disclosure via `capsule explain`

`capsule explain` is the single agent-facing documentation surface. Bare
`capsule explain` prints an index of topics with one-line descriptions and
"load when…" guidance plus task **recipes** (curated topic bundles for common
tasks). `capsule explain <topic> [<topic>…]` loads one or more topics in a
single call. `capsule explain --all` is the escape hatch (offline doc
generation, human reading). The index doubles as routing instructions: an
agent skims it, identifies the matching recipe, and invokes `explain` with
just the topics it needs — paying for only what the current task requires
while keeping greenfield setup to a single `--all` call.

## Considered options

**Monolithic `capsule explain` dump** — one ~400-line markdown blob every
time. Rejected: narrow tasks (rename a stage, debug routing) pay full freight
on every call, and the doc loses the discipline that section boundaries
enforce.

**Callable bundles** (`capsule explain greenfield-setup`) — bundle names
co-exist with topic names in the same namespace. Rejected as unnecessary once
`explain` accepts multiple topics in one call: agents read the recipe in the
index, then load the topics directly. No new namespace to design or police
against drift.

**`.capsule/AGENTS.md` written by `init`** — discoverable in-repo doc. Rejected
as the primary surface: an in-repo file rots against the binary's evolving
grammar, and capsule already controls the binary so versioning content with
the binary is strictly safer. A short pointer comment in the generated
`config.yml` covers in-repo discoverability.
