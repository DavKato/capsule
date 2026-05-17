# Gated release from main

Releases are triggered by `cargo release` on `main` only, gated by CI in the release workflow. No branching strategy (no dev/main split). Changelog is human-curated in `CHANGELOG.md` by a release skill before the release is cut; the GitHub release page uses auto-generated notes as a quick reference.

## Considered Options

- **Merge-triggered release (Model A) with dev/main branching** — every merge to `main` cuts a release, PRs land on `dev`. Rejected: adds permanent branch maintenance cost, merge conflicts on hotfixes, and tag/default-branch weirdness — all for a PR diff view that `git log v0.x.y..main` already provides. The conscious "when to ship" decision is still manual (promoting dev→main), just dressed up as a merge.

- **Changeset files per PR** — each PR drops a `.changes/*.md` file consumed at release time. Rejected: the implementing agent doesn't create the PR, so a separate stage would need to scan the diff — at which point you're doing the same work as release-time synthesis. PRDs already serve as the eager artifact describing user-facing changes.

- **Draft release with post-hoc editing** — workflow creates a draft, human edits after the fact. Rejected: the release skill already presents notes for review before triggering `cargo release`, so the human review happens pre-trigger. A draft adds a redundant step.

- **Release notes parsed from CHANGELOG.md by the workflow** — workflow extracts the latest entry and uses it as the GitHub release body. Rejected: `CHANGELOG.md` is in the repo and linkable; duplicating it into the release page adds coupling with no benefit.

## Guards

- `allow-branch = ["main"]` in cargo-release config — fast local feedback.
- Workflow step verifying the tagged commit is an ancestor of `main` — hard gate against manual tag pushes from feature branches.

## CI deduplication

Standalone CI workflow uses `tags-ignore: ["v*.*.*"]` so it doesn't run on tag pushes. The release workflow calls CI as a reusable workflow dependency, ensuring exactly one CI run per release.
