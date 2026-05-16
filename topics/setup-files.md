# setup-files

Load before editing any `.capsule/` file — maps each file to its ownership, purpose, and edit boundary.

## Quick reference

| File | Owns | When it runs |
|------|------|--------------|
| `config.yml` | Pipeline structure, routing, models, counters | — |
| `Dockerfile` | Container OS packages and tools | At image build time |
| `before-all.sh` | Host-side setup (optional) | Once on host, before any stage |
| `before-each.sh` | Per-invocation container setup (optional) | Inside each container, before prompt |
| `.env` | Env-var defaults and secrets (gitignore) | Sourced on host before `before-all.sh` |
| `prompts/<stage>.md` | Stage prompt content | Mounted into container per invocation |

## config.yml

Central configuration file. Defines stages, loops, routing rules, models, and counters.

- `prompt:` is a path relative to `.capsule/`; conventionally `prompts/<stage-name>.md`.
- Stage names drive prompt filenames — rename a stage, rename its prompt file to match.
- Flat-form config (no `stages:` key) defaults to `prompt.md` in `.capsule/`.
- Run `capsule check` after every structural edit.

## Dockerfile

Extends `FROM capsule` (base image ships `claude`, git, bash). Add runtime deps with `RUN pacman -Syu --noconfirm <pkg>`. Rebuilt on `capsule run --rebuild`.

## before-all.sh

Runs **once on the host** before any container starts. Receives `.env` defaults plus `--env` overrides. Use for host-side bootstrapping: cloning repos, creating GitHub issues, setting up external state. Non-zero exit aborts the run.

## before-each.sh

Runs **inside each container** before Claude reads the prompt. Receives the same env as the stage. Can write to `/home/claude/prompt.txt` to mutate the prompt before the stage sees it. Runs on every invocation — keep it fast.

## .env

Env-var defaults and secrets (`KEY=value`, one per line). Should be gitignored — may hold tokens (e.g. `GH_TOKEN` with `--github-token-from local`). Loaded on the host before `before-all.sh` runs; values flow into all containers and hook scripts. Override at runtime with `capsule run --env KEY=value`.

## Prompt files

One `.md` file per stage. Capsule prepends the system preamble and, where applicable, the previous-stage block before injecting the file into Claude.

- Flat-form default: `.capsule/prompt.md`
- Multi-stage convention: `.capsule/prompts/<stage-name>.md`
- The `prompt:` path in `config.yml` is authoritative — keep it in sync with the actual filename.
