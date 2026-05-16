# pipeline-shapes

Load when choosing between pipeline layouts — single-stage or ralph-loop — or when bootstrapping a new `.capsule/` from a template.

## Shapes at a glance

| Shape | When to use | Loop present |
|-------|-------------|--------------|
| **single-stage** | One-shot task, no review needed, simple output | No |
| **ralph-loop** | Iterative work with implementer+reviewer cycle | Yes |

## single-stage

One stage, no loop. Defined as a `stages:` config with a single named stage.

```yaml
stages:
  - name: main
    prompt: prompt.md
model: claude-sonnet-4-6
github_token_from: local
```

Use when:
- Task is self-contained (generate a file, answer a question, run a script)
- No review gate needed
- Failure just retries from scratch

Bootstrap: `capsule init --template single-stage`

## ralph-loop

A loop containing an implementer+reviewer pair, followed by optional post-loop stages.

```yaml
stages:
  - loop:
      max_iteration: 10
      stages:
        - name: implementer
          prompt: prompts/implementer.md
          on_fail: retry
          max_retries: 3

        - name: reviewer
          prompt: prompts/reviewer.md
          on_fail: implementer

  - name: documentor
    prompt: prompts/documentor.md

model: claude-sonnet-4-6
github_token_from: local
```

Use when:
- Work needs a review gate before continuing
- Implementer may need multiple attempts (`on_fail: implementer` loopback)
- Post-loop stages (PR reviewer, documentor) run once after the loop exits

Bootstrap: `capsule init --template ralph-loop`

## Decision criteria

1. **Does the task need a review gate?** If yes → ralph-loop. If no → single-stage.
2. **Does failure mean "retry the whole thing"?** single-stage with `max_stages: N` is simpler.
3. **Do you need stages after the iterative work?** ralph-loop supports post-loop stages; single-stage does not.
4. **Is the task queue-draining (many items, each simple)?** single-stage per item; orchestrate externally.

## Customizing after init

After `capsule init --template <name>`, edit `config.yml` and `prompts/` to fit your task. Run `capsule check` after every structural change.
