# Issue dependencies API reference

Reference for skills that manage blocking relationships between GitHub issues. Source: <https://docs.github.com/en/rest/issues/issue-dependencies?apiVersion=2026-03-10>.

## The gotcha

Same as sub-issues: the API takes the blocker's **internal database id**, not its issue number.

```bash
gh api repos/$OWNER/$REPO/issues/$NUMBER --jq .id
```

## Endpoints

### Mark an issue as blocked by another

```bash
BLOCKER_ID=$(gh api repos/$OWNER/$REPO/issues/$BLOCKER_NUMBER --jq .id)
gh api repos/$OWNER/$REPO/issues/$BLOCKED_NUMBER/dependencies/blocked_by \
  --method POST \
  -F issue_id=$BLOCKER_ID \
  --silent
```

### List what blocks an issue

```bash
gh api repos/$OWNER/$REPO/issues/$NUMBER/dependencies/blocked_by \
  --jq '.[] | "#\(.number) \(.title)"'
```

### List what an issue is blocking

```bash
gh api repos/$OWNER/$REPO/issues/$NUMBER/dependencies/blocking \
  --jq '.[] | "#\(.number) \(.title)"'
```

### Remove a blocking dependency

```bash
BLOCKER_ID=$(gh api repos/$OWNER/$REPO/issues/$BLOCKER_NUMBER --jq .id)
gh api repos/$OWNER/$REPO/issues/$BLOCKED_NUMBER/dependencies/blocked_by/$BLOCKER_ID \
  --method DELETE \
  --silent
```

## Limits

Up to 50 issues per relationship type (blocked_by, blocking).

## Reading dependency state without extra calls

The `issue_dependencies_summary` field is included on every issue response (including the sub-issues list endpoint). It contains `blocked_by` and `blocking` counts — enough to filter without fetching dependency details.
