# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/DavKato/capsule/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/DavKato/capsule/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/DavKato/capsule/compare/v0.4.2...v0.5.0
