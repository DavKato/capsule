# Right-sized slices for AFK agents

Each `ready-for-agent` slice must be completable by a fresh agent in one session without exceeding ~100kb / 30% context window usage. This replaces the prior "prefer many thin slices" heuristic and removes the interactive quiz from `draft-slices` — the skill now has an objective sizing criterion it can apply autonomously.

Slice *shape* varies by category. Feature work uses tracer-bullet slices — thin vertical paths through every layer (schema, API, UI, tests), each independently demoable. This shape enables TDD: one slice = one red-green-refactor cycle against the module's interface. Bug fixes and refactors don't trace new vertical paths; they produce right-sized work units scoped to the behavioral change. The sizing budget applies to both; the shape differs.

## Considered options

- **Many thin slices + interactive quiz (status quo).** User manually adjusts granularity per batch. Rejected: the quiz adds friction in the `sweep-issues` path, and "thin" is subjective without a cost model.
- **Fixed token budget per slice.** Rejected: token counts vary by model and don't map to what the user observes (context window percentage).
- **Context window usage target (~100kb / 30%).** Chosen: directly maps to agent degradation threshold. Higher usage means more exploration, more re-reading, worse output quality. The constraint is observable and enforceable.

## Consequences

- `draft-slices` no longer needs a quiz step — it sizes autonomously against the budget.
- A single-slice output is valid — not every issue needs splitting.
- The 100kb/30% target is a guideline, not a hard gate. Complex slices may exceed it if they can't be split further without breaking coherence.
