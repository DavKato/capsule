# Environment variables for run-scoped parameterization

Capsule needs a way to pass run-specific values (e.g., a parent issue number
for scoped queue drains) that persist across all stages and hook scripts. We
chose `--env KEY=VALUE` — repeatable CLI flags that inject environment variables
into the container and hook processes — over a custom argument/substitution
system.

## Considered options

**`--arg KEY=VALUE` with `<arg:key>` substitution in prompts** — dedicated
namespace, clear provenance in prompt files. Rejected: requires a substitution
engine in capsule (which ADR-0003 rejected for templates), needs separate
plumbing for shell scripts (which ultimately means environment variables
anyway), and introduces a concept that doesn't compose with existing tools
inside the container.

**A "heap" or shared state file written/read by capsule** — capsule manages a
key-value store that stages can read/write. Rejected: adds a new stateful
concept to the pipeline model, blurs the line between capsule's orchestration
role and stage-level logic, and duplicates what the filesystem (workspace) and
environment already provide.

**Extending `--input` to all stages** — make pipeline input available on every
invocation. Rejected: `--input` is a prompt-injection mechanism (unstructured
text); run-scoped key-value parameters are a different concept. Overloading
`--input` conflates two distinct needs.

## Consequences

- All `--env` values are written to a temp file and passed via `--env-file`,
  never as `-e` args, so values don't leak into `/proc/<pid>/cmdline`.
- `--env` values override same-named keys in `.capsule/.env` (file ordering).
- `CAPSULE_*` keys are rejected to prevent collisions with capsule internals.
- Hook scripts (`before-all.sh`, `before-each.sh`) receive `--env` pairs via
  `Command::env` on the host side.
