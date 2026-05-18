# Capsule is a generic orchestrator; git and GitHub are conveniences

Capsule's identity is "generic multi-stage Claude Code orchestrator in Docker." Git and GitHub integrations (`commit_as`, `github_token_from`) are opt-in conveniences, not structural dependencies. The runtime has no knowledge of GitHub issues, `gh` CLI is only invoked when `github_token_from: global` is explicitly set, and `.git/config` is mounted only when present.

This matters because the README, templates, and glossary all evolved from the author's GitHub-centric usage and gave the impression that capsule is a GitHub automation tool. Going forward, capsule primitives (stages, loops, verdicts, routing) stay domain-agnostic; GitHub-specific patterns (ready-for-agent issues, queue drain) are prompt-level conventions, not capsule concepts.

## Considered options

**GitHub-focused tool** — lean into the GitHub identity, require `gh`, bake issue workflows into the runtime. Rejected: the pipeline engine is already generic; narrowing it would lock out non-GitHub use cases (GitLab, non-code pipelines) for no structural benefit.

**Generic with no git/GitHub support** — remove `commit_as` and `github_token_from` entirely. Rejected: git repos are the dominant use case; these options cost nothing when unused and save real friction when needed.
