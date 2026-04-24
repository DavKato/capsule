# capsule

## Testing

Tests that require a live Docker daemon use `#[requires_docker]` instead of `#[ignore]`:

```rust
#[test]
#[requires_docker]
fn some_test() {
    // test body — no inline guard needed
}
```

**Never use `#[ignore]` on Docker-dependent tests.** The `#[requires_docker]` attribute
injects a runtime guard that makes the test pass silently when Docker is
unavailable (e.g. inside a capsule container) and run fully when it is available
(dev machine, CI with a Docker socket).

The macro lives in `capsule-macros/src/lib.rs`. Test files must have:

```rust
mod common;
use common::requires_docker;  // re-exported from capsule-macros
```

Unit tests (no subprocesses) live inline in `src/` via `#[cfg(test)]`; integration tests live in `tests/`.

## Feedback loops

Before committing, always run:

```sh
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## Versioning

**Never bump the version in `Cargo.toml` manually.** Releases are managed by [`cargo-release`](https://github.com/crate-ci/cargo-release) and the version is set automatically during the release workflow.
