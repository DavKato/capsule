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

Unit tests (no subprocesses) live inline in `src/` via `#[cfg(test)]`; integration tests live in `tests/`.

## Feedback loops

Before committing after the code change, always run:

```sh
cargo fmt
cargo clippy --tests -- -D warnings
cargo test
```

## Stdout routing

All terminal output must go through `capsule::display`. Never use `println!` or `print!` directly in `src/` — use `display::println` / `display::print` instead. The only exceptions are `src/display.rs` (which owns stdout) and `src/mcp_server.rs` (JSON-RPC protocol). CI enforces this via the `lint-stdout` job.

## Releases

**Never edit `CHANGELOG.md` or bump the version in `Cargo.toml` manually.** Both are managed by the `/capsule-release` skill and [`cargo-release`](https://github.com/crate-ci/cargo-release).
