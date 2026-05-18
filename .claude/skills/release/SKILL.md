---
name: release
description: Orchestrate a capsule release — gather PRs, draft changelog, update CHANGELOG.md, run cargo release. Use when cutting a release.
argument-hint: "[patch|minor|major]"
---

# release

Gather merged PRs since last tag, synthesize a changelog entry, get human approval, update `CHANGELOG.md`, and run `cargo release`.

## Workflow

1. **Baseline.** Find the last release tag and its date.

```bash
tag=$(git describe --tags --abbrev=0)
tag_date=$(git log -1 --format=%Y-%m-%d "$tag")
echo "$tag $tag_date"
```

2. **Collect PRs.** List PRs merged after the tag date.

```bash
gh pr list --state merged --search "merged:>$tag_date sort:created-asc" \
  --json number,title,body,labels
```

3. **Extract parent context.** For each PR, find the parent mention in the body. Fetch each parent issue with `gh issue view NNN --json title,body` — the parent describes what the change is *for* at a higher level than the PR. For PRs without a parent, use the PR title and summary directly.

4. **Classify.** Map each PR to a Keep a Changelog category using the PR title prefix, labels, and parent context:

| Category    | Signal                                         |
|-------------|-------------------------------------------------|
| **Added**   | New feature, command, config field              |
| **Changed** | Rename, rework, behavior update                 |
| **Fixed**   | Bug fix, correction                             |
| **Removed** | Deleted flag, removed feature                   |

When ambiguous, prefer **Changed**. Each entry is one concise line describing the user-facing change, not implementation detail. If a change has a migration step (e.g. renamed flag, changed config shape), include it inline after the description.

5. **Recommend bump.** If the user didn't specify a level via `$ARGUMENTS`:
   - **minor** — new features or breaking changes (capsule is pre-1.0; breaking changes go in minor)
   - **patch** — only fixes and non-user-facing changes

   State the recommendation and reasoning.

6. **Present draft.** Show the changelog entry for review:

```markdown
## [VERSION] - YYYY-MM-DD

### Added
- ...

### Changed
- ...
```

Omit empty categories. Use today's date. Compute VERSION from the recommended or specified bump level. Wait for approval or edits before proceeding.

7. **Apply.** On approval, update `CHANGELOG.md`:
   - Clear the `## [Unreleased]` section body (keep the heading)
   - Insert the approved entry between `## [Unreleased]` and the previous release
   - Update comparison links at the bottom:
     - `[Unreleased]` compares from the new version tag to HEAD
     - Add a new line for the released version comparing to the previous tag

   Then run:

```bash
cargo release $level --execute
```

`cargo release` owns the version bump in `Cargo.toml`, commit, tag, and push. Do NOT bump the version manually.
