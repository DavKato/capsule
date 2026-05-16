# Single-stage example

The simplest capsule setup: a single-stage config running one prompt per container. Each run picks one `AFK`-labelled GitHub issue, implements it, and commits.

## Structure

```
.capsule/
  config.yml        # single stage: stages: + prompt, model, github_token_from
  prompt.md         # what Claude does each run
  before-all.sh     # host pre-flight checks (runs once before the first container)
  before-each.sh    # injects AFK issues + recent commits into prompt.txt each run
  Dockerfile        # extends the base capsule image with project-specific tooling
  .env              # GH_TOKEN and other secrets (not committed)
```

## Running it

```sh
capsule init --template single-stage
capsule run
```

Override the model on the fly:

```sh
capsule run --model claude-opus-4-7
```

## Key concepts shown

- **Single-stage config** — `stages:` with one named stage; simplest possible pipeline
- **`before-each.sh`** — injects dynamic context (open AFK issues, recent commits) into `prompt.txt` before each container starts; Claude sees fresh state every run
- **`before-all.sh`** — runs on the host before any container starts; use it for pre-flight checks (services up, env vars set)
- **Custom `Dockerfile`** — extends the base image with project-specific runtimes or tools

For multi-stage pipelines and the full ralph loop pattern, see [`../ralph-loop`](../ralph-loop).
