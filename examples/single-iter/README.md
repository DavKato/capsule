# Single-iteration example

The simplest capsule setup: a flat-form config running a single prompt in a loop. Each iteration picks one `AFK`-labelled GitHub issue, implements it, and commits. The loop runs until the queue is empty or the iteration cap is hit.

## Structure

```
.capsule/
  config.yml        # flat-form: iterations + prompt, no stages:
  prompt.md         # what Claude does each iteration
  before-all.sh     # host pre-flight checks (runs once before the first container)
  before-each.sh    # injects AFK issues + recent commits into prompt.txt each iteration
  Dockerfile        # extends the base capsule image with project-specific tooling
  .env              # GH_TOKEN and other secrets (not committed)
```

## Running it

```sh
cd examples/single-iter
capsule run
```

Override iterations or model on the fly:

```sh
capsule run --iterations 5 --model claude-opus-4-7
```

## Key concepts shown

- **Flat-form config** — `iterations:` + `prompt:` at the top level; no `stages:` needed for single-prompt loops
- **`before-each.sh`** — injects dynamic context (open AFK issues, recent commits) into `prompt.txt` before each container starts; Claude sees fresh state every iteration
- **`before-all.sh`** — runs on the host before any container starts; use it for pre-flight checks (services up, env vars set)
- **Custom `Dockerfile`** — extends the base image with project-specific runtimes or tools

For multi-stage pipelines and the full ralph loop pattern, see [`../ralph-loop`](../ralph-loop).
