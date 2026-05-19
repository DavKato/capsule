---
name: capsule-release
description: Orchestrate a capsule release — gather PRs, draft changelog, update CHANGELOG.md, run cargo release. Use when cutting a release.
argument-hint: "[patch|minor|major]"
---

# release

Gather changes since the last tag, synthesize a changelog entry, get human approval, update `CHANGELOG.md`, and run `cargo release`.

## Workflow

1. **Baseline.** Find the last release tag and its date.

2. **Collect changes.** Run `git log <tag>..HEAD --oneline` to get every commit since the last release. For commits that reference a PR (`(#NNN)`), fetch the PR body. For each PR with a parent issue mention, fetch the parent with `gh issue view NNN --json title,body` — the parent describes what the change is *for* at a higher level. For commits without a matching PR, use the commit message directly.

3. **Classify.** Exclude non-user-facing changes. Map each remaining change to a Keep a Changelog category using the commit/PR title prefix, labels, and parent context:

| Category    | Signal                                         |
|-------------|-------------------------------------------------|
| **Added**   | New feature, command, config field              |
| **Changed** | Rename, rework, behavior update                 |
| **Fixed**   | Bug fix, correction                             |
| **Removed** | Deleted flag, removed feature                   |

When ambiguous, prefer **Changed**. Each change gets its own one-line entry describing the user-facing effect, not implementation detail. If a change has a migration step (e.g. renamed flag, changed config shape), include it inline after the description.

4. **Gate.** If no user-facing changes survive classification, ask the user whether to proceed or bail.

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

   Then commit the changelog change and run `cargo release $level --execute --no-confirm`. `cargo release` owns the version bump in `Cargo.toml`, commit, tag, and push. Do NOT bump the version manually.
