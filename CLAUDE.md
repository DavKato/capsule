# capsule

## Testing

Tests that require a live Docker daemon use a runtime guard instead of `#[ignore]`:

```rust
if !common::docker_available() { return; }
```

**Never use `#[ignore]` on Docker-dependent tests.** The guard makes them pass
silently when Docker is unavailable (e.g. inside a capsule container) and run
fully when it is available (dev machine, CI with a Docker socket).
