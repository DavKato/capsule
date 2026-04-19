use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const SYSTEM_PREAMBLE: &str = include_str!("../templates/system_preamble.md");

pub fn prepend_preamble(user_prompt: &str) -> String {
    format!("{SYSTEM_PREAMBLE}\n\n{user_prompt}")
}

/// Resolve and read the prompt file.
///
/// If `explicit` is `Some`, that path is used. Otherwise the default
/// `${capsule_dir}/prompt.md` is used. Returns the raw file bytes with no
/// transformation. Exits with a clear error naming the expected path when the
/// file cannot be found.
pub fn resolve_prompt(capsule_dir: &Path, explicit: Option<PathBuf>) -> Result<Vec<u8>> {
    let path = explicit.unwrap_or_else(|| capsule_dir.join("prompt.md"));
    std::fs::read(&path).with_context(|| format!("prompt file not found: {}", path.display()))
}
