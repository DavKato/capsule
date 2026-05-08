use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The base Dockerfile embedded at compile time.
pub const DOCKERFILE: &str = include_str!("../base-image/Dockerfile");

/// The container entrypoint script embedded at compile time.
pub const ENTRYPOINT_SH: &str = include_str!("../base-image/entrypoint.sh");

const BASE_IMAGE: &str = "capsule";
const DOCKERFILE_HASH_LABEL: &str = "capsule.dockerfile.hash";

/// FNV-1a hash of a string, formatted as a 16-character hex string.
///
/// Uses hardcoded FNV-1a constants for stability across Rust versions
/// (DefaultHasher is not guaranteed stable).
pub fn fnv1a_hash(content: &str) -> String {
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
    /// Whether to force a rebuild, ignoring cached layers.
    pub rebuild: bool,
    /// The capsule directory — contains an optional `Dockerfile` and is used as
    /// the build context for derived images.
    pub capsule_dir: PathBuf,
    /// The current working directory — used to derive the image name for derived
    /// images (`capsule-<basename(pwd)>`).
    pub pwd: PathBuf,
}

/// Build the base `capsule` Docker image from the embedded Dockerfile.
///
/// Skips the build when the image exists and its stored hash matches the
/// embedded Dockerfile. Auto-rebuilds (with layer cache) when the hash
/// differs. With `rebuild: true`, always rebuilds using `--no-cache`.
pub fn build_base_image(cfg: &BuildConfig) -> Result<()> {
    let hash = fnv1a_hash(&format!("{DOCKERFILE}{ENTRYPOINT_SH}"));

    if !cfg.rebuild && image_exists(BASE_IMAGE) {
        if image_label(BASE_IMAGE, DOCKERFILE_HASH_LABEL).as_deref() == Some(&hash) {
            return Ok(());
        }
        eprintln!("Base Dockerfile changed — rebuilding {BASE_IMAGE}…");
    } else {
        eprintln!("Building {BASE_IMAGE} image…");
    }

    let ctx = tempfile::tempdir().context("failed to create build context tempdir")?;
    std::fs::write(ctx.path().join("Dockerfile"), DOCKERFILE)
        .context("failed to write Dockerfile to build context")?;
    std::fs::write(ctx.path().join("entrypoint.sh"), ENTRYPOINT_SH)
        .context("failed to write entrypoint.sh to build context")?;

    let ctx_path = ctx.path().to_string_lossy().into_owned();
    let label = format!("{DOCKERFILE_HASH_LABEL}={hash}");
    let mut build_args = vec!["build", "-t", BASE_IMAGE, "--label", &label];
    if cfg.rebuild {
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

    eprintln!("Image ready.");
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
        eprintln!("Derived Dockerfile changed — rebuilding {name}…");
    } else {
        eprintln!("Building derived image {name}…");
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

    eprintln!("Derived image ready.");
    Ok(Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Embedded assets ───────────────────────────────────────────────────────

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
