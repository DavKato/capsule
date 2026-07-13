# Sub-issues API reference

Reference for skills that touch GitHub's sub-issues REST API. Read by **Persist**-phase skills only (currently `create-issue`; future `reparent-issue`). Source: <https://docs.github.com/en/rest/issues/sub-issues?apiVersion=2026-03-10>.

## The gotcha

The sub-issues API takes the sub-issue's **internal database id**, not its issue number. The id is the integer in the `id` field of any issue's JSON, _not_ the `number` field that appears in URLs and `gh issue view`. You almost always have the issue number and need to resolve it to an id before mutating.

```bash
# Resolve issue number → database id
gh api repos/$OWNER/$REPO/issues/$NUMBER --jq .id
```

## Endpoints

### Attach a sub-issue to a parent

```bash
gh api repos/$OWNER/$REPO/issues/$PARENT_NUMBER/sub_issues \
  -F sub_issue_id=$CHILD_DB_ID \
  --jq '{id: .id, total_sub_issues: .sub_issues_summary.total}'
```

The parent is identified by `number` (path param). The child is identified by database `id` (body param). The raw response is the entire parent issue JSON — always pipe through `--jq` to avoid context bloat.

### Reparent a sub-issue (move to a different parent)

```bash
gh api repos/$OWNER/$REPO/issues/$NEW_PARENT_NUMBER/sub_issues \
  -F sub_issue_id=$CHILD_DB_ID \
  -F replace_parent=true \
  --jq '{id: .id, total_sub_issues: .sub_issues_summary.total}'
```

`replace_parent=true` detaches the child from its current parent before attaching to the new one. Without this flag, the call fails if the child already has a parent. Use `-F` (not `-f`) for both `sub_issue_id` and `replace_parent` — `-f` sends strings, but the API expects an integer and a boolean respectively.

### Detach a sub-issue (without reparenting)

```bash
gh api repos/$OWNER/$REPO/issues/$PARENT_NUMBER/sub_issue \
  -X DELETE \
  -F sub_issue_id=$CHILD_DB_ID \
  --jq '{id: .id, total_sub_issues: .sub_issues_summary.total}'
```

Note the path is singular `sub_issue`, not `sub_issues`, on the DELETE endpoint.

### List sub-issues of a parent

```bash
gh api repos/$OWNER/$REPO/issues/$PARENT_NUMBER/sub_issues
```

Pagination: `per_page` (max 100, default 30), `page` (default 1).

### Get the parent of a sub-issue

```bash
gh api repos/$OWNER/$REPO/issues/$NUMBER/parent
```

Returns the parent issue, or 404 if the issue has no parent.

### Reorder sub-issues under a parent

```bash
gh api repos/$OWNER/$REPO/issues/$PARENT_NUMBER/sub_issues/priority \
  -X PATCH \
  -F sub_issue_id=$CHILD_DB_ID \
  -F after_id=$OTHER_CHILD_DB_ID \
  --jq '{id: .id, total_sub_issues: .sub_issues_summary.total}'
```

`after_id` _or_ `before_id` is required (one, not both). Both take database ids, not numbers.

## Finding issues without a parent

GitHub's search qualifier finds orphan candidates without per-issue API calls:

```bash
gh issue list --search "no:parent-issue" --json number,title,labels
```

Returns all issues that have no parent. Filter locally with `jq` to narrow further (e.g. issues also missing a state label).

## Drain signal

The condition signaling that all child work under a parent is complete: zero open sub-issues under that parent.

```bash
gh api repos/$OWNER/$REPO/issues/$PARENT_NUMBER/sub_issues \
  --jq '[.[] | select(.state == "open")] | length'
```

Returns `0` when the parent is drained.

## Resolving owner and repo

Derive `$OWNER/$REPO` from the current git remote rather than hardcoding:

```bash
gh repo view --json nameWithOwner --jq .nameWithOwner
```

## Rate limits

The GitHub docs warn that rapid creation or deletion of sub-issues may trigger secondary rate limiting. Workflows that batch many mutations (e.g. filing 10 sub-issues in sequence or adopting a backlog of orphans) should pace themselves — a brief sleep between calls is fine.

## Limits

Per the API docs as of March 2026, sub-issue nesting is supported up to a depth of 8 levels and approximately 50 sub-issues per parent. Both far exceed what this repo's pipeline needs (a parent + flat list of slices, depth 2).
