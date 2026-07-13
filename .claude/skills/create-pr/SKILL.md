---
name: create-pr
description: Creates a GitHub PR with a standard body and default reviewer. Use when opening a pull request.
phase: persist
argument-hint: [title] [parent-issue-number]
---

# create-pr

Creates a PR on the current branch with a standard body shape.

## Template

Title is required. Body:

```
## Parent
Closes #<number>  ← omit section if no parent issue

## Summary
<what changed and why, 1–2 sentences>

## Notes
<anything reviewers should know>  ← omit section if empty
```

## Behaviour

- Creates against the repo default branch unless the caller specifies a base.
- Print the resulting PR URL when done.
