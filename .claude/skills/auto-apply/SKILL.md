---
name: auto-apply
description: Applies all recommendations from the most recent discovery output without asking for per-item confirmation. Use after a discovery skill when changes should be applied automatically.
phase: meta
---

# auto-apply

Behavior modifier. When invoked (directly or by an integration skill), apply every recommendation from the most recent discovery output in conversation without asking the user for per-item confirmation.

## What this means

- Read the latest findings / audit / recommendation list in conversation.
- Apply each item directly: edit the file, post the comment, mutate the issue, etc., according to what the finding says.
- Do not ask "should I apply finding #N?" between items. The user's invocation is the consent.
- Continue until all findings are applied, then summarize what was applied and what (if anything) was skipped, with reasons.

## When to skip an individual finding

Only skip if the finding is malformed or genuinely ambiguous (references a file that no longer exists, contradicts another finding, action target unclear). Note skips in the summary; do not silently drop them.

## Composing with discovery skills

Integration skills typically chain this after a discovery primitive:

1. Run `audit-X` → findings list in conversation.
2. Run `auto-apply` → all findings applied without prompts.
3. Summarize.
