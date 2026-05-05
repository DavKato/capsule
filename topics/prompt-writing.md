# prompt-writing

Load when authoring a stage prompt — to understand the verdict contract, note-injection mechanics, or how to open a prompt so the stage knows its role.

## Verdict contract

Every stage must call `submit_verdict(status, notes)` exactly once before ending its turn. Capsule reads this call as the stage's output; everything else is invisible to the pipeline.

```python
# Emit at the end of every execution path — including early exits
submit_verdict(status="pass", notes="Implemented feature X. Committed as abc1234.")
submit_verdict(status="fail", notes="Could not reproduce the build error after 3 attempts.")
submit_verdict(status="done", notes="All acceptance criteria met. No further iterations needed.")
```

| Status | When to use |
|--------|-------------|
| `pass` | Stage task completed successfully; pipeline continues routing |
| `fail` | Stage task failed; routes per `on_fail` (default: exit) |
| `done` | Scope is complete — exits the nearest enclosing loop or pipeline |

Never omit `submit_verdict`. A stage that ends without calling it is treated as an implicit `fail`.

## `done` vs `pass`

- Use `pass` when your task for this iteration is complete but the loop should continue (e.g., implementer finishes; reviewer still needs to run).
- Use `done` when the full scope is resolved and no further iterations are needed (e.g., reviewer confirms all criteria met; no point running another cycle).
- `done` inside a loop exits that loop; execution continues with the next pipeline entry. At top level, `done` exits the entire pipeline with success.
- `pass` at the end of a loop body starts the next iteration — it does not exit the loop.

## Note injection

The `notes` argument to `submit_verdict` becomes the `<previous-stage>` block injected at the top of the *next* stage's prompt:

```xml
<previous-stage>
Stage: reviewer
Status: pass
Notes: Reviewed commit abc1234. Two issues fixed: ... Committed as def5678.
</previous-stage>
```

Write notes as a factual summary of what you did and what the next stage needs to know. Capsule injects this verbatim — it is the only structured information that crosses the stage boundary. If notes is empty, no block is injected.

## Role framing

Open each stage prompt with a one-sentence statement of the stage's role and its single responsibility. This prevents drift and keeps the stage from overreaching.

```markdown
You are an implementation agent.

Your job: implement the sub-issue described below, commit the result, and call submit_verdict.
```

Or for a reviewer:

```markdown
You are a code reviewer.

Your job: review the implementation referenced in <previous-stage>, identify any issues, and either approve (pass) or send it back for revision (fail with notes).
```

## Prompt skeleton

```markdown
You are a <role> agent.

<one-sentence responsibility statement>

<!-- context: what the stage receives -->
<!-- task: what the stage must do -->
<!-- completion signal: when and how to call submit_verdict -->

When your task is complete, call submit_verdict exactly once:
- status: "pass" if succeeded, "fail" if not, "done" if the scope is fully resolved
- notes: a brief summary of what was done or why it failed
```

Keep the skeleton short. Stages receive a fresh context each invocation — avoid padding with information available via tools (`capsule explain`, file reads).
