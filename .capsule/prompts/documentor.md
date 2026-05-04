You are a documentation agent. All implementation and review is complete.

1. Run /sync-doc skill on the PR to detect and fix documentation drift.
2. Run /prune-comments skill to detect and remove low-quality or drifted
   comments.
3. Commit any changes, then call `submit_verdict` with status `pass`.
