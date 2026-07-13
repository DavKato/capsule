#!/usr/bin/env bash
set -euo pipefail

PARENT_NUMBER=$1
REPO=$(gh repo view --json nameWithOwner --jq .nameWithOwner)

gh api "repos/$REPO/issues/$PARENT_NUMBER/sub_issues" \
  --paginate \
  --jq '[.[] | {
    number,
    title,
    state,
    labels: [.labels[].name],
    assignees: [.assignees[].login],
    dependencies: .issue_dependencies_summary
  }]'
