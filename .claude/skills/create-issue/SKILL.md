---
name: create-issue
description: File a GitHub issue using a canonical template. Use when filing an issue, or as the filing primitive composed by other skills.
phase: persist
---

# create-issue

Files a GitHub issue from conversation context.

## Batching

When the caller has multiple issues to file, handle them all in a single invocation. Read docs (labels, sub-issues, templates) once, then loop `gh issue create` for each issue. Do not ask the caller to invoke this skill repeatedly.

## Workflow

1. **Choose a template** from `templates/`. If conversation makes it obvious, use it. Otherwise recommend and confirm with the user. Read the chosen template only when needed.
2. **Render** the template using conversation context.
3. **Resolve refs.** Infer parent and blockers from conversation context. Only ask if genuinely ambiguous. If the user names an issue by keyword, follow up with `gh issue view <n>` or `gh issue list --search "..."` — never scan the full list.
4. **Resolve labels** per `${CLAUDE_SKILL_DIR}/../docs/labels.md`. Default category label = template name (except `slice` → none). Default state label = `needs-triage` unless the caller specified otherwise (slices must have a state label from the caller — `ready-for-agent` or `ready-for-human`).
5. **File** with `gh issue create`. Do NOT ask the user to review the drafted body first.
6. **Attach as sub-issue** if a parent was given. Resolve the new issue's database id and the parent's number, then call the attach endpoint per `${CLAUDE_SKILL_DIR}/../docs/sub-issues.md`.
7. **Associate blockers** if any were resolved in step 3. For each blocker, call the blocked-by endpoint per `${CLAUDE_SKILL_DIR}/../docs/issue-dependencies.md`.
8. **Print the URL.**

## Body rules

- **No file paths or line numbers** — they go stale.
- **Use the project's domain language** (run `read-domain` if not already loaded in conversation).
- **Describe behaviors, not code.**
