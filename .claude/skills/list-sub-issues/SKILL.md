---
name: list-sub-issues
description: List sub-issues of a GitHub parent issue. Use when you need to see the sub-issues under a given issue number.
phase: discovery
argument-hint: <parent-issue-number>
arguments: parent_number
model: claude-haiku-4-5
---

# list-sub-issues

List sub-issues under a parent GitHub issue.

```bash
bash ${CLAUDE_SKILL_DIR}/scripts/list.sh $parent_number
```

Surface the output as a list in conversation.
