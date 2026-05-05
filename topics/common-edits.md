# common-edits

Load when making structural changes to `config.yml` — renaming stages, adding or removing stages, adding hooks, or changing routing rules.

## Rename a stage

1. Change `name:` in `config.yml`.
2. Rename the prompt file: `prompts/<old>.md` → `prompts/<new>.md`.
3. Update `prompt:` in `config.yml` to the new path.
4. Update any `on_fail:` or `on_pass:` references to the old name.
5. Run `capsule check`.

## Add a stage

1. Insert a `name:` block at the correct position in `stages:`.
2. Create `.capsule/prompts/<stage-name>.md`.
3. Set `on_fail:` / `on_pass:` if defaults (`exit` / fall-through) are wrong.
4. Run `capsule check`.

```yaml
stages:
  - loop:
      max_iteration: 10
      stages:
        - name: implementer
          prompt: prompts/implementer.md
          on_fail: retry
        - name: reviewer
          prompt: prompts/reviewer.md
          on_fail: implementer
  - name: documentor          # new post-loop stage
    prompt: prompts/documentor.md
```

## Remove a stage

1. Delete the `name:` block from `config.yml`.
2. Remove any `on_fail:` or `on_pass:` references to that name.
3. Delete the prompt file if no longer referenced.
4. Run `capsule check`.

## Add a hook

Create `.capsule/before-all.sh` (runs once on host before any container) or `.capsule/before-each.sh` (runs inside each container before the prompt). Both must be executable (`chmod +x`). No `config.yml` change needed — capsule discovers hooks by filename.

```sh
#!/usr/bin/env bash
set -euo pipefail
# before-all.sh: host-side setup (clone repos, create issues, set external state)
# before-each.sh: can write /home/claude/prompt.txt to mutate the prompt
```

Run `capsule check` after adding a hook.

## Change routing

Edit `on_fail:` or `on_pass:` on the relevant stage. Valid targets: a stage name, `retry`, `exit`, or `next`.

```yaml
# Send reviewer failures back to implementer instead of exiting
- name: reviewer
  prompt: prompts/reviewer.md
  on_fail: implementer   # was: exit
```

Run `capsule check` after every routing change.

## Adjust loop counters

```yaml
- loop:
    max_iteration: 10       # total passes before cap-hit; increase for longer tasks
    stages:
      - name: implementer
        prompt: prompts/implementer.md
        on_fail: retry
        max_retries: 3      # consecutive fail retries; increase to allow more self-correction
```
