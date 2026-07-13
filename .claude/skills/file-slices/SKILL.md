---
name: file-slices
description: File an approved slice list as a chain of GitHub issues with a working branch and parent linkage. Composes draft-slices and create-issue. Use when the user wants to convert a plan into issues end-to-end.
phase: integration
---

# file-slices

End-to-end path from a plan in conversation to a chain of filed slice issues with a working branch.

## Workflow

1. **Distill if needed.** If the conversation already contains a slice list, reuse it. Otherwise invoke /draft-slices.

2. **File slices in dependency order** (blockers first so their issue numbers exist for dependency association). Run `/create-issue`.

3. **Post working branch comment** on the parent issue per `${CLAUDE_SKILL_DIR}/../docs/working-branch.md`. Skip if the parent already has a working branch comment.

4. **Print the resulting issue URLs** as a chain.

Do NOT close or modify the parent issue body.
