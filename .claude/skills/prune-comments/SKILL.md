---
name: prune-comments
description: Audits low-value comments and removes them in one pass. Composes audit-comments + auto-apply. Use when tidying up code comments in a repo.
phase: integration
---

# prune-comments

Audits and removes low-value comments in one pass.

## Workflow

1. Invoke /audit-comments over the user's scope (or repo-wide if none specified). This emits a numbered findings list.
2. If zero findings, exit early with "nothing to prune."
3. Invoke /auto-apply against the findings. Each finding is removed (or partially removed) according to its recommendation.
4. Summarize: files touched, comments removed by category.
5. Offer to commit the changes with a message like `chore: prune low-value comments`.

The user's invocation of `prune-comments` is the consent for removal — `auto-apply` does not pause per-finding.
