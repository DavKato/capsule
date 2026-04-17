/// Returns true if a Docker daemon is reachable.
/// Tests that require Docker call this at the top and return early if false.
/// Do NOT replace this guard with #[ignore] — see CLAUDE.md.
pub fn docker_available() -> bool {
    std::process::Command::new("docker")
        .args(["info"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
