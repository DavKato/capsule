pub use capsule_macros::requires_docker;

/// Returns true if a Docker daemon is reachable.
/// Kept for reference; `#[requires_docker]` inlines equivalent logic.
/// Do NOT replace Docker-dependent tests with `#[ignore]` — see CLAUDE.md.
#[allow(dead_code)]
pub fn docker_available() -> bool {
    std::process::Command::new("docker")
        .args(["info"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
