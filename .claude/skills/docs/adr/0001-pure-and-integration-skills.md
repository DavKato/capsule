# Pure and integration skills

Every skill declares a `phase:` in frontmatter and lives as either a **Pure skill** (single phase: discovery, distillation, persist, execute, or meta) or an **Integration skill** that composes other skills. We chose this over monolithic skills that span phases internally because composability is what makes skills maintainable as a codebase — small testable units we can chain explicitly, mirroring what works in code.

## Considered options

- **Monolithic skills (status quo before this decision).** One skill does discovery + distillation + persist (e.g. old `file-improvement`). Rejected: the trailing persist locks in the next action and prevents alternative chains like "implement on the spot" instead of "file an issue."
- **Action-agnostic skills without decomposition.** Strip the persist tails but keep skills that span discovery + distillation. Rejected: the spanning still hides composition opportunities and makes individual skills harder to reuse from integration skills.
- **Full decomposition into pure skills + named integration skills (chosen).** Each skill stays in one phase; integration skills are explicit and named. Workflow becomes user-driven chaining, optionally automated by integration skills like `sweep-issues`.

## Consequences

- The skill count grows substantially (pure primitives extracted from former multi-phase skills).
- Workflows that were one skill invocation become chains; mitigated by user-only integration skills that compose the chain cheaply (~10 tokens of global cost each because their description doesn't need to support model invocation).
- Work-in-progress can be lost between skill invocations if the session is cleared (artifacts live in conversation, not in durable storage). Mitigated by `sweep-issues` and per-feature resume wrappers.
- `write-a-skill` enforces the `phase:` contract; new skills cannot bypass it.
