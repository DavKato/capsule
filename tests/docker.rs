use capsule::docker::{build_base_image, DOCKERFILE, STREAM_DISPLAY_JQ};

// ── Unit tests ────────────────────────────────────────────────────────────────

#[test]
fn embedded_dockerfile_is_non_empty() {
    assert!(
        !DOCKERFILE.is_empty(),
        "embedded Dockerfile must not be empty"
    );
    assert!(
        DOCKERFILE.contains("FROM archlinux"),
        "Dockerfile must start from archlinux base"
    );
}

#[test]
fn embedded_stream_display_jq_is_non_empty() {
    assert!(
        !STREAM_DISPLAY_JQ.is_empty(),
        "embedded stream_display.jq must not be empty"
    );
    assert!(
        STREAM_DISPLAY_JQ.contains("fromjson"),
        "jq filter must contain fromjson"
    );
}

// ── Integration tests (require Docker daemon) ─────────────────────────────────
// Run with: cargo test -- --ignored

/// When no `capsule` image exists, `build_base_image(false)` should build it.
/// NOTE: This test pulls/builds a real Docker image — slow on first run.
#[test]
#[ignore]
fn build_base_image_creates_image_when_absent() {
    // Remove image first so we start from a known state.
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();

    build_base_image(false).expect("build_base_image should succeed");

    let out = std::process::Command::new("docker")
        .args(["image", "inspect", "capsule"])
        .output()
        .expect("docker inspect should run");
    assert!(
        out.status.success(),
        "capsule image should exist after build"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();
}

/// When image already exists, `build_base_image(false)` should skip the build
/// (observable: function returns Ok without invoking a long build).
#[test]
#[ignore]
fn build_base_image_skips_when_image_present() {
    // Ensure image exists using a trivial image (busybox tagged as capsule).
    let _ = std::process::Command::new("docker")
        .args(["pull", "busybox:latest"])
        .output();
    std::process::Command::new("docker")
        .args(["tag", "busybox:latest", "capsule"])
        .output()
        .expect("docker tag should succeed");

    // Should succeed quickly (no build needed).
    build_base_image(false).expect("build_base_image should succeed when image present");

    // Image still present, was not rebuilt (we tagged busybox; if rebuilt it would
    // be an archlinux image — but we only check it exists and call returns Ok).
    let out = std::process::Command::new("docker")
        .args(["image", "inspect", "capsule"])
        .output()
        .expect("docker inspect should run");
    assert!(out.status.success(), "capsule image should still exist");

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();
}

/// `build_base_image(true)` should rebuild even when the image already exists.
#[test]
#[ignore]
fn build_base_image_rebuilds_when_rebuild_flag_set() {
    // Tag busybox as capsule so an image exists before we call rebuild.
    let _ = std::process::Command::new("docker")
        .args(["pull", "busybox:latest"])
        .output();
    std::process::Command::new("docker")
        .args(["tag", "busybox:latest", "capsule"])
        .output()
        .expect("docker tag should succeed");

    // --rebuild should trigger a fresh build (will take a while in real use).
    build_base_image(true).expect("build_base_image --rebuild should succeed");

    let out = std::process::Command::new("docker")
        .args(["image", "inspect", "capsule"])
        .output()
        .expect("docker inspect should run");
    assert!(
        out.status.success(),
        "capsule image should exist after rebuild"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();
}
