# Capsule

Capsule is a CLI tool for orchestrating multi-stage Claude pipelines. A `config.yml` describes the execution graph; `capsule run` executes it.

## Language

### Execution structure

**Pipeline**:
The ordered execution graph described by a `config.yml`, built from stages and loops.
_Avoid_: Workflow, process, script

**Run**:
One invocation of `capsule run` executing a pipeline end-to-end.
_Avoid_: Session, execution, process

**Stage**:
A single Claude invocation with a prompt, model, and routing rules; produces one verdict.
_Avoid_: Step, phase, task

**Loop**:
A `loop:` block wrapping an ordered list of stages that re-enters from the top until a `done` verdict or `max_iteration`.
_Avoid_: Iteration block, subflow

**Loop body**:
The ordered stages inside a loop.
_Avoid_: Loop contents

**Top-of-body stage**:
The first stage in a loop body; its re-entries tick `max_iteration`.
_Avoid_: Loop head, entry stage

**Scope**:
The nearest enclosing loop, or the pipeline if no loop encloses the stage; the target a `done` verdict exits.
_Avoid_: Context, frame

**Iteration**:
One pass through a loop body, starting at the top-of-body stage.
_Avoid_: Pass, cycle, round

### Verdicts

**Verdict**:
Claude's structured signal at stage completion, delivered via the MCP tool: `{status, notes}`.
_Avoid_: Result, outcome, report

**Pass**:
Verdict status meaning "succeeded"; routes per `on_pass`.
_Avoid_: Success, ok

**Fail**:
Verdict status meaning "something went wrong"; routes per `on_fail`.
_Avoid_: Error, reject

**Done**:
Verdict status meaning "this scope is complete and clean"; exits the nearest enclosing scope cleanly, overriding `on_pass`/`on_fail`.
_Avoid_: Complete, finished, drained

**Notes**:
Optional free-text field on a verdict; carried forward via note injection.
_Avoid_: Feedback, message, comment

**Implicit fail**:
A stage exiting its container without emitting a verdict is treated as `fail` with a diagnostic note.
_Avoid_: Silent fail, default fail

### Routing

**Route target**:
A value accepted by `on_pass` / `on_fail`: a stage name, `exit`, or `retry`.
_Avoid_: Destination, handler

**Fall-through**:
Implicit routing when `on_pass` is unset: next entry in surrounding `stages:`; at end of a loop body → next iteration; at end of pipeline → pipeline success.
_Avoid_: Default routing

**Loopback**:
Explicit routing backward within a pipeline or loop body (e.g., reviewer's `on_fail: implementer`).
_Avoid_: Goto, rewind

### Counters

**`max_iteration`**:
Per-loop cap; ticks on every top-of-body entry except self-`retry`.
_Avoid_: Loop budget

**`max_retries`**:
Per-stage cap on consecutive `fail` verdicts; resets on `pass`; independent of loop position.
_Avoid_: Retry limit

**`max_stages`**:
Global circuit breaker across all stage invocations in a run; top-level only.
_Avoid_: Total budget, max_pipeline_iterations

**Cap-hit**:
Any counter exceeding its cap; pipeline terminates non-zero, writes summary artifact, no `on_fail` routing applies.
_Avoid_: Overflow, timeout

### Input, injection, and state

**Pipeline input**:
String passed via `capsule run --input "..."`, injected into the first stage's prompt on its first invocation only.
_Avoid_: Argument, seed

**Run environment**:
User-supplied `KEY=VALUE` pairs passed via `--env` on `capsule run`, injected into every container invocation and hook script for the duration of the run. Persisted in pipeline state on resumable exits so `capsule resume` restores them; `--env` on resume merges on top (last writer wins per key). Distinct from pipeline input (prompt-only, first invocation only) and `.capsule/.env` (gitignored defaults and secrets). Written to a temp file and passed via `--env-file` so values never appear in process arguments.
_Avoid_: Run args, run parameters, heap

**Note injection**:
Capsule's prepending of a block containing the previous stage's name, verdict, and notes to every next-stage prompt.
_Avoid_: Feedback block, context injection

**Previous-stage block**:
The capsule-owned block produced by note injection; contains verdict status and verbatim notes with no capsule-authored directive.
_Avoid_: Feedback block, preamble

**Iteration boundary**:
The rule that every stage invocation runs in a fresh Claude context with no session continuity.
_Avoid_: Session reset

**Workspace**:
The bind-mounted working directory shared by all stages; equivalent to the host's `pwd`.
_Avoid_: Sandbox, mount

**External state**:
Task/work state stored outside capsule (GitHub issues, files on disk); the source of truth for queue-drain workflows.
_Avoid_: User state

**Summary artifact**:
`.capsule/last-run.json` written on every pipeline exit, recording terminal reason, last stage, last verdict, counters, and session ID; includes pipeline state only on mid-session exits.
_Avoid_: Exit log, run record

**Pipeline state**:
The runtime executor state (stage index, counters, fail counts, loop positions) serialized to the summary artifact only on mid-session exits to enable `capsule resume`.
_Avoid_: Checkpoint, executor state

### Credentials and recovery

**Credential copy**:
An isolated copy of `~/.claude/.credentials.json` held in a temp file for the run's lifetime, preventing concurrent token rotation races between host and container.
_Avoid_: Snapshot, credential isolation

**Resume-retry**:
On `authentication_failed`, capsule re-copies the host's current credentials and re-launches with `claude --resume <session_id>`, recovering conversation context in one attempt.
_Avoid_: Auth retry, credential refresh


### Workflow patterns

**Ready-for-agent issue**:
A GitHub issue labeled `ready-for-agent`, eligible for capsule to pick up autonomously.
_Avoid_: AFK issue, bot issue, capsule issue

**Ready-for-human issue**:
A GitHub issue labeled `ready-for-human`, reserved for the human to work on interactively.
_Avoid_: HITL issue, manual issue

**Queue drain**:
The workflow pattern in which a loop's implementer repeatedly picks the next ready-for-agent issue, emitting `done` when the queue is empty.
_Avoid_: Worklist, backlog drain

**Dream cycle**:
The end-to-end autonomous flow: plan → queue drain (implement ↔ review) → document, wrapped in one pipeline.
_Avoid_: Full auto

**Fan-out** *(out of scope)*:
A single stage producing N independent executions in parallel, one per item in an upstream list; not part of the current pipeline grammar.
_Avoid_: Parallel, shard

### Setup and discovery *(planned — surface not yet implemented)*

**Template**:
A pre-built `.capsule/` skeleton (e.g., `single-iter`, `ralph-loop`) shipped in the capsule repo's `templates/` directory; `capsule init --template <name>` copies it byte-for-byte into the user's repo. Templates replace the former `examples/` directory and are validated in CI via `capsule check`.
_Avoid_: Example, scaffold, preset, starter

**`capsule init`**:
Greenfield bootstrap command. Bare `capsule init` is human-interactive (prompts for template choice); agents take the deterministic path `capsule templates list` → `capsule init --template <name>`. Refuses to overwrite an existing `.capsule/` without `--force`. Non-TTY stdin fails fast with a redirect to the agent path.
_Avoid_: Setup, scaffold, generate

**`capsule templates list`**:
Agent-facing enumeration of available templates with one-line descriptions. The deterministic counterpart to interactive `init`.
_Avoid_: Template index

**`capsule check`**:
Validates a `.capsule/` setup. Level 2 structural checks (route targets resolve, prompt files exist, hook scripts present, loop nesting rules hold) plus opportunistic Level 3 hints (typo suggestions, missing-env-var warnings). The feedback loop that makes "edit + validate" a viable migration workflow.
_Avoid_: Validate, lint, verify

**`capsule explain`**:
Agent-facing documentation surface using progressive disclosure. Bare `capsule explain` prints the **index** (topic names, one-line descriptions, "load when…" guidance, **recipes**); `capsule explain <topic> [<topic>…]` loads one or more **topics** in a single call; `capsule explain --all` dumps everything.
_Avoid_: Docs, help, manual

**Topic**:
An atomic unit of `capsule explain` content (e.g., `mental-model`, `setup-files`, `pipeline-shapes`, `prompt-writing`, `common-edits`, `commands`). Topics are loaded on demand by name.
_Avoid_: Section, chapter, page

**Recipe**:
A hand-curated entry in the `capsule explain` index that maps a common task (e.g., "rename a stage") to the ordered list of topics an agent should load for that task. Recipes are guidance only, not callable; the agent reads the recipe and invokes `capsule explain <topic1> <topic2> …` itself.
_Avoid_: Bundle, preset, playbook

**Skill**:
A two-rule Claude Code routing signpost — "for setup, run `capsule templates list`; for customization/debugging/migration, run `capsule explain`." Contains no embedded content; all content lives in `capsule explain`.
_Avoid_: Plugin, agent definition

**Base image assets**:
The compile-time-bundled files in the repo's `base-image/` directory (`Dockerfile`, `entrypoint.sh`, `system_preamble.md`) — embedded into the `capsule` binary via `include_str!`. Not user-facing; not a **Template**.
_Avoid_: Templates (the directory was renamed in 2026-04 to free the name for user scaffolds)

## Relationships

- A **Pipeline** contains one or more **Stages** and zero or more **Loops** in its top-level `stages:`.
- A **Loop** contains one or more **Stages** in its own `stages:`; nested loops are rejected at config load.
- A **Stage** produces exactly one **Verdict** per invocation.
- A **Verdict** has a status (**Pass** | **Fail** | **Done**) and optional **Notes**.
- **Done** exits the nearest enclosing **Scope** (a **Loop** or the **Pipeline**).
- Each **Run** executes one **Pipeline** and writes one **Summary artifact** on exit.
- **Note injection** fires on every stage-to-stage transition regardless of verdict status; a missing `notes` field suppresses the **Previous-stage block** entirely.

## Example dialogue

> **Dev:** "If the reviewer keeps rejecting the same task, does that count against `max_iteration` or `max_retries`?"
>
> **Domain expert:** "Both. Each time the reviewer fails back to the implementer, the implementer is re-entering the top-of-body, so `max_iteration` ticks. And the reviewer's own `max_retries` ticks on each fail — those reset when the reviewer eventually passes."
>
> **Dev:** "Got it. So when the queue is empty, the implementer should emit `done`, not `fail`?"
>
> **Domain expert:** "Right — `done` is the clean scope-exit. Inside the loop it exits just the loop; the pipeline continues to the documentor. If the implementer emitted `fail` instead, routing would fire `on_fail` — default `exit` — and the whole run would terminate non-zero."
>
> **Dev:** "And the documentor still sees the implementer's `done` notes via note injection?"
>
> **Domain expert:** "Yes. Note injection is uniform — pass, fail, or done handoffs all prepend a previous-stage block. If the implementer emitted no notes on `done`, the block is omitted."

## Flagged ambiguities

- **"Iteration"** always means one pass through a loop body. Flat-form config (which used `iterations:` as a top-level keyword) was removed; all configs now use `stages:`.
- **"Pipeline" vs "process" vs "run"**: Canonical: **Pipeline** = the configured execution graph; **Run** = one invocation executing it. Avoid "process" (conflates with OS processes).
- **"Scope"**: Canonical: **Scope** = nearest enclosing loop or pipeline (the thing a `done` verdict exits). Not a synonym for "workflow shape."
- **"Feedback"** was renamed: the verdict field is **Notes**; the delivery mechanism is **Note injection** / **Previous-stage block**. Drop "feedback" from new code and docs.
- **"Exit"** is a route target (config-level, non-zero pipeline termination), not a verdict. Say "the stage emitted `done`" or "the pipeline exited with 0 / non-zero" — not "the stage exited cleanly."
- **"Fan-out"**: Canonical: **Fan-out** = parallel shard execution, out of scope. Sequential iteration over a list is a **Queue drain** using a **Loop**.
- **"Templates"**: Canonical: **Template** = user-facing `.capsule/` skeleton under `templates/` (planned). The compile-time-bundled files under `base-image/` are **base image assets**, not templates — the directory was renamed in 2026-04 to remove the overload.
- **"Examples"**: The former `examples/` directory will be replaced by `templates/` once the new surface ships; "example" should not be used for canonical scaffolds going forward — say **template**.
