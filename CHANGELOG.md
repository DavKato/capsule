# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Config-driven `setup` field replaces `before-*.sh` hooks

### Changed

- Renamed `single-iter` template to `single-stage`
- Renamed `--git-identity` to `--commit-as`
- Renamed `--github` to `--github-token-from`
- Summary artifact schema: removed `terminal_reason` value `"ok"`, renamed `cap_hit_kind` value `"max_pipeline_iterations"` to `"max_stages"`

### Removed

- `--iterations` (replaced by `max_iteration` in config)
- `--prompt` (replaced by `prompt` field in stage config)
- `--min-token-lifetime-minutes` (credential lifetime is now managed internally)

[Unreleased]: https://github.com/DavKato/capsule/compare/v0.4.1...HEAD
