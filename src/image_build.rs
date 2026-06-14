use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The base Dockerfile embedded at compile time.
pub const DOCKERFILE: &str = include_str!("../base-image/Dockerfile");

/// The container entrypoint script embedded at compile time.
pub const ENTRYPOINT_SH: &str = include_str!("../base-image/entrypoint.sh");

/// Git wrapper that re-forces capsule's configured identity on every git call.
pub const GIT_WRAPPER_SH: &str = include_str!("../base-image/git-wrapper.sh");

const BASE_IMAGE: &str = "capsule";
const DOCKERFILE_HASH_LABEL: &str = "capsule.dockerfile.hash";

/// The upstream OS image the base `capsule` image is built from (e.g.
/// `ubuntu:24.04`), parsed from the embedded Dockerfile.
///
/// `base-image/Dockerfile` is the single source of truth for the base distro;
/// callers that need the raw upstream image (e.g. tests) derive it from here
/// rather than hardcoding it, so swapping the base OS is a one-line change.
///
/// Returns the image of the *last* `FROM` (the stage that ships in a
/// multi-stage build, == the only stage in a single-stage one) with any
/// `AS <alias>` suffix and `--flag` options stripped.
pub fn upstream_base_image() -> &'static str {
    parse_base_image(DOCKERFILE).expect("base-image/Dockerfile must contain a FROM line")
}

/// Extract the shipping stage's base image from Dockerfile text. See
/// [`upstream_base_image`] for the selection rules.
fn parse_base_image(dockerfile: &str) -> Option<&str> {
    dockerfile
        .lines()
        .filter_map(|line| line.trim().strip_prefix("FROM "))
        .next_back()
        .and_then(|rest| rest.split_whitespace().find(|tok| !tok.starts_with("--")))
}

/// FNV-1a hash of a string, formatted as a 16-character hex string.
///
/// Uses hardcoded FNV-1a constants for stability across Rust versions
/// (DefaultHasher is not guaranteed stable).
fn fnv1a_hash(content: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn image_label(name: &str, label: &str) -> Option<String> {
    let out = Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            &format!("{{{{index .Config.Labels \"{label}\"}}}}"),
            name,
        ])
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn image_exists(name: &str) -> bool {
    Command::new("docker")
        .args(["image", "inspect", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Returns the derived image name for the given working directory.
///
/// Format: `capsule-<basename(pwd)>`. Falls back to `capsule-project` when the
/// directory has no file-name component (e.g. `/`).
pub fn derived_image_name(pwd: &std::path::Path) -> String {
    let basename = pwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    format!("capsule-{basename}")
}

/// Configuration for Docker image building operations.
#[derive(Clone)]
pub struct BuildConfig {
    pub rebuild: bool,
    pub capsule_dir: PathBuf,
    pub pwd: PathBuf,
}

/// Build the base `capsule` Docker image from the embedded Dockerfile.
///
/// Skips the build when the image exists and its stored hash matches the
/// embedded Dockerfile. Auto-rebuilds (with layer cache) when the hash
/// differs. With `rebuild: true`, always rebuilds using `--no-cache`.
pub fn build_base_image(rebuild: bool) -> Result<()> {
    let hash = fnv1a_hash(&format!("{DOCKERFILE}{ENTRYPOINT_SH}{GIT_WRAPPER_SH}"));

    if !rebuild && image_exists(BASE_IMAGE) {
        if image_label(BASE_IMAGE, DOCKERFILE_HASH_LABEL).as_deref() == Some(&hash) {
            return Ok(());
        }
        crate::display::capsule_info(&format!(
            "Base Dockerfile changed — rebuilding {BASE_IMAGE}…"
        ));
    } else {
        crate::display::capsule_info(&format!("Building {BASE_IMAGE} image…"));
    }

    let ctx = tempfile::tempdir().context("failed to create build context tempdir")?;
    std::fs::write(ctx.path().join("Dockerfile"), DOCKERFILE)
        .context("failed to write Dockerfile to build context")?;
    std::fs::write(ctx.path().join("entrypoint.sh"), ENTRYPOINT_SH)
        .context("failed to write entrypoint.sh to build context")?;
    std::fs::write(ctx.path().join("git-wrapper.sh"), GIT_WRAPPER_SH)
        .context("failed to write git-wrapper.sh to build context")?;

    let ctx_path = ctx.path().to_string_lossy().into_owned();
    let label = format!("{DOCKERFILE_HASH_LABEL}={hash}");
    let mut build_args = vec!["build", "-t", BASE_IMAGE, "--label", &label];
    if rebuild {
        build_args.push("--no-cache");
    }
    build_args.push(&ctx_path);

    let status = Command::new("docker")
        .args(&build_args)
        .status()
        .context("failed to spawn `docker build`")?;
    if !status.success() {
        bail!(
            "docker build exited with code {}",
            status.code().unwrap_or(-1)
        );
    }

    crate::display::capsule_info("Image ready.");
    Ok(())
}

/// Build a derived Docker image from `${capsule_dir}/Dockerfile` if it exists.
///
/// Returns `Ok(None)` when no `Dockerfile` is found in `capsule_dir`.
/// Returns `Ok(Some(name))` with the derived image name when the image exists or
/// was successfully built.
///
/// The derived image is named `capsule-<basename(pwd)>` and uses `capsule_dir`
/// as its build context so relative `COPY` instructions resolve correctly.
///
/// If `rebuild` is `false` and the derived image already exists, the build is
/// skipped and the cached image name is returned.
pub fn build_derived_image(cfg: &BuildConfig) -> Result<Option<String>> {
    let dockerfile = cfg.capsule_dir.join("Dockerfile");
    if !dockerfile.exists() {
        return Ok(None);
    }

    let name = derived_image_name(&cfg.pwd);
    let content = std::fs::read_to_string(&dockerfile)
        .with_context(|| format!("failed to read {}", dockerfile.display()))?;
    let hash = fnv1a_hash(&content);

    if !cfg.rebuild && image_exists(&name) {
        if image_label(&name, DOCKERFILE_HASH_LABEL).as_deref() == Some(&hash) {
            return Ok(Some(name));
        }
        crate::display::capsule_info(&format!("Derived Dockerfile changed — rebuilding {name}…"));
    } else {
        crate::display::capsule_info(&format!("Building derived image {name}…"));
    }

    let label = format!("{DOCKERFILE_HASH_LABEL}={hash}");
    let status = Command::new("docker")
        .args([
            "build",
            "-t",
            &name,
            "--label",
            &label,
            "-f",
            &dockerfile.to_string_lossy(),
            &cfg.capsule_dir.to_string_lossy(),
        ])
        .status()
        .context("failed to spawn `docker build` for derived image")?;

    if !status.success() {
        bail!(
            "docker build for derived image {name} exited with code {}",
            status.code().unwrap_or(-1)
        );
    }

    crate::display::capsule_info("Derived image ready.");
    Ok(Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Embedded assets ───────────────────────────────────────────────────────

    #[test]
    fn parse_base_image_handles_single_stage_and_alias_and_flags() {
        assert_eq!(
            parse_base_image("FROM ubuntu:24.04\n"),
            Some("ubuntu:24.04")
        );
        assert_eq!(
            parse_base_image("FROM ubuntu:24.04 AS runtime\n"),
            Some("ubuntu:24.04")
        );
        assert_eq!(
            parse_base_image("FROM --platform=$BUILDPLATFORM ubuntu:24.04\n"),
            Some("ubuntu:24.04")
        );
        // Multi-stage: the shipping (last) stage wins.
        assert_eq!(
            parse_base_image("FROM rust:1 AS builder\nRUN cargo build\nFROM ubuntu:24.04\n"),
            Some("ubuntu:24.04")
        );
        assert_eq!(parse_base_image("# no from here\n"), None);
    }

    #[test]
    fn upstream_base_image_matches_embedded_dockerfile() {
        // Sanity: the real Dockerfile parses to a concrete, pullable image ref.
        assert!(upstream_base_image().contains(':'));
    }

    #[test]
    fn embedded_entrypoint_is_non_empty() {
        assert!(
            !ENTRYPOINT_SH.is_empty(),
            "embedded entrypoint.sh must not be empty"
        );
    }

    // ── fnv1a_hash ────────────────────────────────────────────────────────────

    #[test]
    fn fnv1a_hash_is_deterministic() {
        let input = "FROM archlinux\nRUN pacman -Syu\n";
        assert_eq!(fnv1a_hash(input), fnv1a_hash(input));
    }

    #[test]
    fn fnv1a_hash_collision_resistance() {
        // Different inputs must produce different hashes.
        assert_ne!(fnv1a_hash("hello"), fnv1a_hash("world"));
        assert_ne!(fnv1a_hash("FROM archlinux"), fnv1a_hash("FROM ubuntu"));
        // Single-byte difference must change the hash.
        assert_ne!(fnv1a_hash("abcde"), fnv1a_hash("abcdf"));
    }

    #[test]
    fn fnv1a_hash_known_value_for_empty_string() {
        // FNV-1a offset basis with no bytes processed = cbf29ce484222325
        assert_eq!(fnv1a_hash(""), "cbf29ce484222325");
    }

    #[test]
    fn fnv1a_hash_output_is_16_hex_chars() {
        let h = fnv1a_hash("any content here");
        assert_eq!(h.len(), 16, "expected 16 hex chars, got: {h}");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()), "not all hex: {h}");
    }

    // ── derived_image_name ────────────────────────────────────────────────────

    #[test]
    fn derived_image_name_uses_basename_of_pwd() {
        let dir = tempfile::tempdir().expect("temp dir");
        let project_dir = dir.path().join("my-project");
        std::fs::create_dir(&project_dir).unwrap();
        let name = derived_image_name(&project_dir);
        assert_eq!(name, "capsule-my-project");
    }

    #[test]
    fn derived_image_name_handles_root_or_unnamed() {
        let name = derived_image_name(std::path::Path::new("/"));
        assert!(name.starts_with("capsule-"), "name={name}");
    }

    // ── build_derived_image (no Docker) ───────────────────────────────────────

    #[test]
    fn build_derived_image_returns_none_when_no_dockerfile() {
        let capsule_dir = tempfile::tempdir().expect("temp dir");
        let pwd = tempfile::tempdir().expect("temp dir");
        let cfg = BuildConfig {
            rebuild: false,
            capsule_dir: capsule_dir.path().to_path_buf(),
            pwd: pwd.path().to_path_buf(),
        };
        let result = build_derived_image(&cfg).expect("should not error when Dockerfile absent");
        assert!(result.is_none(), "expected None when no Dockerfile");
    }
}
