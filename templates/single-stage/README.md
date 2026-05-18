# Single-stage example

The simplest capsule setup: a single-stage config running one prompt per container. Each run picks an open GitHub issue, implements it, and commits.

## Structure

```
.capsule/
  config.yml        # single stage: stages: + prompt, model, github_token_from, setup
  prompt.md         # what Claude does each run
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
- **`setup` field** — runs a command or script before the stage starts; use for injecting dynamic context or installing dependencies
- **Custom `Dockerfile`** — extends the base image with project-specific runtimes or tools

For multi-stage pipelines and the full ralph loop pattern, see [`../ralph-loop`](../ralph-loop).
