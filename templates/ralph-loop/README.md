# Ralph loop example

A reference pipeline demonstrating the ralph loop pattern: an implement ↔ review feedback loop that drains a queue of `AFK`-labelled GitHub issues, followed by a holistic PR review and a documentation pass.

The pattern is named after Ralph Wiggum — relentless persistence until external criteria confirm the work is done.

## Pipeline shape

```
loop [ implementer ◄──► reviewer ] ──► pr-reviewer ──► documentor
```

| Stage | Role | Verdict |
|---|---|---|
| implementer | Works through `AFK` issues one at a time | `done` when queue is empty, `fail` if stuck |
| reviewer | Reviews the latest commit | `pass` to continue, `fail` routes back to implementer |
| pr-reviewer | Holistic review of all changes once the loop exits | `pass` to proceed, `fail` opens new `AFK` issues and routes back to implementer |
| documentor | Updates docs to reflect the completed work | `pass` |

## How routing works

- **implementer → reviewer**: fall-through after each issue
- **reviewer → implementer** (on fail): loopback via `on_fail: implementer`; reviewer notes arrive in implementer's next prompt as a `<previous-stage>` block
- **implementer → pr-reviewer** (queue empty): `done` verdict exits the loop scope and falls through to the next top-level stage
- **pr-reviewer → implementer** (on fail): loopback via `on_fail: implementer`; new `AFK` issues opened by pr-reviewer get picked up on re-entry
- **Loop cap**: `max_iteration: 10` prevents runaway loops

## Running it

Seed the run with a task description via `--input`:

```sh
cd examples/ralph-loop
capsule run --input "Add a health-check endpoint to the API"
```

The implementer will pick up any open `AFK` issues. Use `--input` to inject context at the start of the first stage's prompt.

For run-scoped parameters that persist across all stages and hooks (e.g. filtering issues by parent), use `--env`:

```sh
capsule run --env PARENT=79    # $PARENT is available in every container and hook script
```

## Key concepts shown

- **Ralph loop** — implementer iterates until the `AFK` queue is empty, using `done` to signal completion rather than `pass`
- **Note injection** — reviewer feedback is automatically prepended to implementer's next prompt as a `<previous-stage>` block
- **`done` for queue drain** — `done` exits the nearest enclosing scope (the loop) cleanly; `pass` would trigger another iteration
- **`on_fail: retry` + `max_retries`** — guards against implementer getting stuck in a hard failure without consuming all loop iterations
- **Holistic PR review** — pr-reviewer runs once after the loop, catching cross-cutting issues the per-commit reviewer might miss
- **Prompt files** — each stage has its own file under `prompts/`; paths are relative to `.capsule/`
