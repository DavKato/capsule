# setup-files

Load before editing any `.capsule/` file — maps each file to its ownership, purpose, and edit boundary.

## Quick reference

| File / Field | Owns | When it runs |
|------|------|--------------|
| `config.yml` | Pipeline structure, routing, models, counters, setup | — |
| `Dockerfile` | Container OS packages and tools | At image build time |
| `setup` (top-level) | Host-side setup command or script (optional) | Once on host, before any stage |
| `setup` (per-stage) | Per-invocation container setup (optional) | Inside each container, before prompt |
| `docker.volumes` | Host volumes to bind-mount into containers | At `docker run` invocation |
| `.env` | Env-var defaults and secrets (gitignore) | Sourced on host before top-level `setup` |
| `prompts/<stage>.md` | Stage prompt content | Mounted into container per invocation |

## config.yml

Central configuration file. Defines stages, loops, routing rules, models, and counters.

- `prompt:` is a path relative to `.capsule/`; conventionally `prompts/<stage-name>.md`.
- Stage names drive prompt filenames — rename a stage, rename its prompt file to match.
- Run `capsule check` after every structural edit.

## Dockerfile

Extends `FROM capsule` (base image ships `claude`, git, bash). Add runtime deps with `RUN apt-get update && apt-get install -y --no-install-recommends <pkg>`. Rebuilt on `capsule run --rebuild`.

## setup (top-level)

Runs **once on the host** before any container starts. Receives `.env` defaults plus `--env` overrides. Use for host-side bootstrapping: cloning repos, creating GitHub issues, setting up external state. Non-zero exit aborts the run.

The value can be an inline shell command (contains whitespace) or a path to a script file relative to `.capsule/`.

```yaml
setup: scripts/bootstrap.sh
```

## setup (per-stage)

Runs **inside each container** before Claude reads the prompt. Receives the same env as the stage. Can write to `/home/claude/prompt.txt` to mutate the prompt before the stage sees it. Runs on every invocation — keep it fast.

```yaml
stages:
  - name: main
    prompt: prompts/main.md
    setup: pip install -r requirements.txt
```

## docker.volumes

Bind-mounts host directories into every container invocation. Specified under a `docker:` block at top level (applies to all stages) or per stage (merged on top of top-level volumes).

```yaml
# Top-level: mounted in every stage
docker:
  volumes:
    - /host/data:/container/data
    - /host/models:/models:ro

stages:
  - name: main
    prompt: prompts/main.md
    # Per-stage: merged with top-level volumes
    docker:
      volumes:
        - ./local:/workspace/local
```

Relative source paths (e.g. `./local`) are resolved against the host workspace (`pwd`). Absolute paths pass through unchanged. Format follows Docker's `-v` syntax: `host-path:container-path[:opts]`.

## .env

Env-var defaults and secrets (`KEY=value`, one per line). Should be gitignored — may hold tokens (e.g. `GH_TOKEN` with `--github-token-from local`). Loaded on the host before the top-level `setup` runs; values flow into all containers and setup commands. Override at runtime with `capsule run --env KEY=value`.

## Prompt files

One `.md` file per stage. Capsule prepends the system preamble and, where applicable, the previous-stage block before injecting the file into Claude.

- Convention: `.capsule/prompts/<stage-name>.md`
- The `prompt:` path in `config.yml` is authoritative — keep it in sync with the actual filename.
