# mental-model

Load when you need to understand how capsule executes a pipeline — verdict semantics, routing rules, scope exits, or counter behavior.

## Execution primitives

- **Pipeline**: the top-level execution graph defined in `config.yml`. Contains an ordered list of entries (stages and loops). Runs top-to-bottom unless routing overrides.
- **Stage**: one Claude invocation. Takes a prompt, produces exactly one verdict, then terminates. Every stage runs in a fresh context — no session continuity between invocations.
- **Loop**: a `loop:` block wrapping an ordered list of stages. Re-enters from the top until a `done` verdict or a counter cap.

## Verdicts

Each stage must call `submit_verdict(status, notes)` exactly once before ending its turn.

| Status | Meaning | Routing effect |
|--------|---------|----------------|
| `pass` | Stage succeeded | Routes per `on_pass` (default: fall-through) |
| `fail` | Stage failed | Routes per `on_fail` (default: `exit`) |
| `done` | Scope complete | Exits the nearest enclosing scope immediately — ignores `on_pass`/`on_fail` |

If a stage exits without calling `submit_verdict`, capsule treats it as an implicit `fail`.

## Routing

- **`on_pass`**: `next` (default, fall-through), a stage name, or `exit`.
- **`on_fail`**: `exit` (default), `retry` (re-run same stage), or a stage name.
- **Fall-through**: when `on_pass` is unset, advance to the next entry in the surrounding `stages:` list. At end of a loop body → next iteration. At end of pipeline → pipeline success.
- **Loopback**: routing backward to a prior stage (e.g., `on_fail: implementer` on a reviewer). The target re-runs in a fresh context with the previous verdict injected.

## Scope semantics

A `done` verdict exits the nearest enclosing **scope**:

- Inside a loop → exits that loop; pipeline continues with the next entry after the loop.
- At top level (no enclosing loop) → exits the entire pipeline with success.

`done` is the only clean way to terminate a loop. `pass` at end-of-body starts the next iteration; it does not exit.

## Iteration boundary

Every stage invocation starts a fresh Claude context. There is no conversation continuity between stages or between iterations of the same stage. The only information that crosses the boundary is:

1. **Note injection** — capsule prepends a `<previous-stage>` block to each prompt containing the prior stage's name, verdict status, and notes.
2. **Workspace** — the shared filesystem (bind-mounted `pwd`). Stages communicate durable state through files.

If the previous verdict had no notes (or notes was empty), no `<previous-stage>` block is injected.

## Counters

| Counter | Scope | Ticks on | Cap-hit behavior |
|---------|-------|----------|------------------|
| `max_iteration` | Per loop | Every top-of-body entry (except self-`retry`) | Pipeline terminates non-zero |
| `max_retries` | Per stage | Each consecutive `fail` verdict (resets on `pass`) | Pipeline terminates non-zero |
| `max_pipeline_iterations` | Global | Every stage invocation in the run | Pipeline terminates non-zero |

On any cap-hit: the pipeline stops immediately, writes a summary artifact, and exits non-zero. No `on_fail` routing applies — cap-hits bypass routing entirely.

## Control flow diagram

```
Pipeline
├─ Stage A ──pass──→ Stage B ──pass──→ ...
│                        │
│                     on_fail: A  (loopback)
│
├─ Loop (max_iteration: N)
│   ├─ Implementer ──pass──→ Reviewer
│   │       ↑                    │
│   │       └── on_fail ─────────┘
│   │
│   └─ (done from any stage) ──→ exits loop
│
└─ Documentor ──pass──→ pipeline success
```
