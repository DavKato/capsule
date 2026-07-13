# Working Branch

The working branch is declared via a comment on the parent issue. The branch itself is created at pickup time by the first implementing agent.

## Comment format

```
> **Working branch:** `<branch-name>`
> Base: `<base-branch>`
```

## Naming convention

`<category>/<parent-issue-number>-<slugified-title>` — e.g. `feat/42-user-auth`, `fix/87-session-crash`.

## Rules

- **Declare at filing time, create at pickup time.** The comment declares intent; the branch is created from the current base when an agent starts work.
- **One comment per parent.** Sub-issues inherit the branch by reading the parent.
- **Standalone issues get their own comment.** If an issue has no parent, the working branch comment goes on the issue itself.
- **Resolve the base branch.** If the parent issue has a working branch comment, use the parent's branch as the base. Otherwise default to `main`. Only ask when neither applies.
