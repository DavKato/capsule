---
name: draft-slices
description: Synthesize a plan, PRD, bug report, refactor RFC, or PR-review into right-sized slices ready for AFK agents. Output is the slice list in conversation; no files are touched.
phase: distillation
---

# draft-slices

Takes the current conversation (a plan, PRD, bug report, refactor RFC, PR-review findings, QA breakdown, etc.) and produces a numbered list of right-sized slices in conversation. A single-slice output is valid — not every issue needs splitting. Does not file anything — that's a downstream step.

## Workflow

1. **Ground in the codebase.** Invoke /read-domain, then briefly explore the repo to understand the affected modules. Skip exploration if conversation context already covers it.

2. **Draft right-sized slices.** Each slice must be completable by a fresh agent in one session within ~100kb / 30% context window usage. The overhead of filing an issue, spinning up an agent, and opening a PR must not dwarf the actual work — bundle items that are too small to justify that overhead. Slice shape varies by category:
   - **Feature work** — tracer-bullet slices: thin vertical paths through every layer (schema, API, UI, tests), each independently demoable. This shape enables TDD: one slice = one red-green-refactor cycle.
   - **Bug fixes / refactors** — right-sized work units scoped to the behavioral change. No vertical tracing needed.

3. **Emit the list.** Present one block per slice with title, blockers, state (`ready-for-agent` or `ready-for-human`), and a body suitable for the slice template (see `${CLAUDE_SKILL_DIR}/../create-issue/templates/slice.md`). That template is the source of truth for slice body structure — do not duplicate the section list here.
