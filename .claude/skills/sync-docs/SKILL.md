---
name: sync-docs
description: Audits docs against a code diff and applies all recommended fixes in one pass. Composes audit-docs (diff scope) + auto-apply. Use after a code change to update docs.
phase: integration
---

# sync-docs

Audits docs against a code diff and applies all recommended fixes in one pass.

## Workflow

1. Invoke /audit-docs with the user-specified diff scope (commit, PR, commit range, "since main", etc.). If scope is unclear, ask. This emits a numbered findings list.
2. If zero findings, exit early with "nothing to sync."
3. Invoke /auto-apply against the findings. Each finding is fixed or filled in according to its recommendation.
4. Summarize: files touched, findings applied by severity, any skipped (with reasons).
5. Offer to commit the changes with a message like `docs: sync with <scope>`.

The user's invocation of `sync-docs` is the consent — `auto-apply` does not pause per-finding.
