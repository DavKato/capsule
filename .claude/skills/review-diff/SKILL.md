---
name: review-diff
description: Reviews a diff against security, correctness, testability, and maintainability criteria, emitting a ranked findings list with recommended fixes. Use when reviewing a PR, commit, or branch.
phase: discovery
argument-hint: [pr-number|commit|branch]
when_to_use: User wants to review a PR, commit hash, branch diff, or "last commit"; audit a change set; check a diff for issues; or mentions "pr-review".
---

# review-diff

Reviews a diff and emits a findings list with recommended fixes. Does not implement — that's downstream.

## Workflow

### 1 — Locate the diff

Accept any of: PR number, commit hash, branch name, "last commit", or a range. Fetch the changed file list (`--name-only`) and any available metadata (title, description).

If the PR body references a parent issue or PRD, fetch it too — it gives intent context that sharpens the review. State the resolved parent issue number in the findings output so downstream skills (e.g. `file-slices`) can use it without re-resolving.

### 2 — Scope assessment

Group **implementation files only** by top-level directory (≈ module). Test files are excluded from scope assignment — they are unscoped reference material that any scout can read during exploration.

- Large diff across multiple directories: **one group per directory**. Merge small groups.
- Small diff or few directories: **one group**. Skip to step 5 — the orchestrator handles it directly.

If scope assessment produces **a single group**, skip steps 3 and 4. Read the diff, read the checklist at `${CLAUDE_SKILL_DIR}/CHECKLIST.md`, explore the codebase, and produce findings directly in step 5.

**Drift check:** Run `git diff HEAD --stat`. If non-empty, the working tree has changes beyond the diff under review. Include this warning in each scout's prompt (step 4): "The working tree has uncommitted changes beyond this PR diff. Code/diff mismatches may reflect local edits, not bugs in the PR."

### 3 — Split the diff

Run the split script to divide the diff by scope group:

```bash
${CLAUDE_SKILL_DIR}/scripts/split-diff.sh <pr-number> "group1:file1,file2" "group2:file3,file4" ...
```

The script fetches the full diff, splits it by the groups from step 2, writes per-group temp files, and prints their paths — one per line. The orchestrator never loads the diff into its own context.

### 4 — Review scouts

Spawn one `review-scout` agent per scope group, all in parallel.

**Each scout prompt must include:**

- The temp file path for its diff slice (from step 3)
- Its assigned file list
- Path to the checklist: `${CLAUDE_SKILL_DIR}/CHECKLIST.md`
- PR metadata from step 1 (title, description, linked issue context)
- If drift was detected in step 2, the drift warning

Scouts return observations (tagged with checklist checkpoints) and open questions. See the `review-scout` agent definition for the full contract.

### 5 — Produce findings

**Multi-group path:** Collect all scout reports. Cross-reference observations and resolve open questions — e.g. if one scout reports "deleted tests, absorption target not in my scope," check whether another scout's observations confirm replacement coverage exists.

**Single-group path:** You have the full diff and codebase context. Evaluate against the checklist directly.

Produce a numbered findings list. Each finding:

| Field               | Content                                |
| ------------------- | -------------------------------------- |
| **Title**           | Short label                            |
| **Severity**        | Critical / High / Medium / Low         |
| **Checkpoint**      | Which category triggered this          |
| **Description**     | What the problem is and why it matters |
| **Recommended fix** | Concrete, actionable suggestion        |

Only include real findings — do not pad. If a checkpoint is clean, skip it.

End with a one-line summary: total findings, breakdown by severity. Do not modify any file.
