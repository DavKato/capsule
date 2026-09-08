# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.1] - 2026-09-08

### Fixed

- Containers now carry the host user's supplementary groups (`--group-add`), so bind-mounted resources reachable through group membership on the host — such as `/var/run/docker.sock` via `docker.volumes` — are accessible inside the container
- The capsule skill is packaged as `skills/capsule/SKILL.md` so the skills CLI finds it; the README install command now points at the correct path

## [0.8.0] - 2026-05-21

### Added

- `docker.volumes` config option for mounting host directories into containers, with support for top-level and per-stage volumes and relative path resolution against the workspace

### Changed

- Containers now run as the host user's uid/gid (`--user`) instead of hardcoded uid 1000, eliminating permission issues with bind-mounted files

## [0.7.2] - 2026-05-21

### Added

- JSON Schema for `config.yml` with editor autocomplete — init templates now include a `yaml-language-server` schema comment so editors pick up validation and completion automatically

## [0.7.1] - 2026-05-21

### Fixed

- `commit_as` identity now works for direct git invocations inside capsules (the v0.6.1 fix only covered the git wrapper)

## [0.7.0] - 2026-05-21

### Added

- Buffered sub-agent display — each sub-agent shows a live progress line that collapses to a one-line summary on completion
- `max_failure` config field — caps total failures per stage, independent of retry counting

### Changed

- `max_retries` default raised from 3 to 10; only counts when `on_fail: retry`
- Forced pipeline exits now explain which stage hit which limit
- Tool result duration now appears on the tool call line, not a separate line

## [0.6.1] - 2026-05-19

### Fixed

- `commit_as` identity is now enforced via a git wrapper inside the container, preventing the agent from overriding the configured author

## [0.6.0] - 2026-05-19

### Added

- Visual grouping of sub-agent tool calls under their parent `Agent` call using tree-drawing prefixes (`├─`)

### Fixed

- Standardized `[dev]` line indentation in terminal output
- Show skill names instead of raw arguments in tool call logs
- Suppressed extra blank lines between list items across tool calls
- Deduplicated Done lines for repeated tool call IDs

## [0.5.0] - 2026-05-18

### Added

- Config-driven `setup` field replaces `before-*.sh` hooks — top-level `setup` runs on host, per-stage `setup` runs inside the container
- `--max-stages` flag sets a global ceiling on total stage invocations (replaces `--iterations`)

### Changed

- Flat-form config is no longer supported — a `config.yml` without `stages:` now produces a hard error with migration example
- Renamed CLI flags: `--git_identity` → `--commit-as`, `--github` → `--github-token-from`
- Renamed config fields to match CLI: `git_identity` → `commit_as`, `github` → `github_token_from`, `max_pipeline_iterations` → `max_stages`
- Renamed `single-iter` template to `single-stage`
- Token lifetime check is now a non-blocking warning instead of a configurable blocking prompt
- `capsule resume` cannot resume runs from v0.4.x due to `last-run.json` schema changes

### Removed

- `--prompt` flag (use `prompt` field in stage config)
- `--min-token-lifetime-minutes` flag (credential lifetime is now managed internally)
- `before-all.sh` / `before-each.sh` convention (presence now triggers a migration error)

[Unreleased]: https://github.com/DavKato/capsule/compare/v0.8.1...HEAD
[0.8.1]: https://github.com/DavKato/capsule/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/DavKato/capsule/compare/v0.7.2...v0.8.0
[0.7.2]: https://github.com/DavKato/capsule/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/DavKato/capsule/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/DavKato/capsule/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/DavKato/capsule/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/DavKato/capsule/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/DavKato/capsule/compare/v0.4.2...v0.5.0
