# Persist run environment across resume

Run environment (`--env KEY=VALUE` pairs) is a property of the Run, not the CLI
invocation — the glossary defines it as lasting "for the duration of the run,"
and `capsule resume` continues the same Run. We persist `--env` pairs in
`last-run.json` and auto-restore them on resume, overriding PRD #93's original
"no `--env` on resume" stance.

## Considered options

**Do not persist; resume runs without env pairs** — the PRD #93 position.
Avoids putting user-supplied values on disk beyond the run's lifetime. Rejected:
resumed stages silently lose environment they may depend on, violating the
glossary definition of Run environment. The "ambiguity" the PRD aimed to avoid
is a design-time ambiguity, while the ambiguity it creates is a runtime
correctness failure.

**Warn on resume but don't restore** — store a count of original env pairs in
`last-run.json` and print a warning. Rejected: makes the failure visible but
doesn't fix it; the user still can't resume correctly without re-running from
scratch.

## Consequences

- `pipeline_state` in `last-run.json` gains an `env` field (array of
  `[key, value]` pairs). Because `pipeline_state` is only written on resumable
  exits (`FailExit`, `CapHit`, errors), env pairs are never persisted after
  successful runs. Existing files without the field default to `[]`.
- `capsule resume` accepts `--env KEY=VALUE` for overrides. Merge semantic:
  persisted pairs are the base, `--env` flags on resume override per-key
  (last writer wins). No `--unset-env` mechanic; users can set a key to empty
  or edit `last-run.json` directly.
- Security posture is unchanged: `last-run.json` sits in `.capsule/`, same
  trust boundary as `.capsule/.env` (same user, gitignored). Env values are
  only on disk between a failed run and the next run.
- Hook idempotency is an existing contract, unaffected by this change.
  `before-all.sh` re-fires on resume with the restored (possibly overridden)
  env pairs.
