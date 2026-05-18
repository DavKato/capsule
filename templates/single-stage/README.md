# Single-stage template

The simplest capsule setup: one stage with a short starter prompt for a fresh,
unconfigured project. It points the user toward `capsule explain` instead of
assuming any issue queue, branch rules, or commit process.

## Structure

```
.capsule/
  config.yml        # single stage: stages: + prompt, model, github_token_from
  prompt.md         # brief setup hint for a fresh project
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
- **Starter prompt** — a brief default message that sends users to `capsule explain` for setup guidance
- **Custom `Dockerfile`** — extends the base image with project-specific runtimes or tools

For multi-stage pipelines and the full ralph loop pattern, see [`../ralph-loop`](../ralph-loop).
