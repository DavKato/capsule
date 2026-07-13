# Lazy branch creation at pickup time

The working branch name is decided at filing time and posted as a comment on the parent issue. The branch itself is created when the first agent picks up work — from the base branch declared in the comment. This decouples branch creation from filing and eliminates staleness (branches created days or weeks before work begins diverge from `main`).

## Considered options

- **Eager creation at filing time (status quo).** `file-slices` creates the branch immediately. Rejected: the branch goes stale between filing and pickup, forcing rebases before any real work begins.
- **No branch name until pickup.** The pickup agent both names and creates the branch. Rejected: knowing the branch name at filing time is useful for planning and cross-referencing.
- **Declare name at filing, create at pickup (chosen).** A comment on the parent issue declares the branch name and base. The first implementing agent creates the branch from the current base. Forward-compatible with a future move to git worktrees.

## Consequences

- `file-slices` no longer creates branches — it posts a working branch comment on the parent issue.
- `sweep-issues` posts the same comment format during enrichment.
- The pickup agent checks whether the branch exists and creates it if not.
- Sub-issues inherit the branch name from the parent per ADR-0002.
- The "Working branch" section is removed from the slice template — branch info lives on the parent, not on each slice.
