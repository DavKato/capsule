use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::templates;

pub fn init(template: &str, dest: &Path, force: bool) -> Result<()> {
    if dest.exists() {
        if !force {
            bail!(".capsule/ already exists. Use --force to overwrite existing setup.");
        }
        let parent = dest.parent().unwrap_or(Path::new("."));
        let staging = tempfile::tempdir_in(parent)
            .context("failed to create staging directory for atomic swap")?;
        let staging_capsule = staging.path().join(".capsule");
        templates::copy_to(template, &staging_capsule)?;
        let backup = tempfile::tempdir_in(parent)
            .context("failed to create backup directory for atomic swap")?;
        let backup_path = backup.path().join(".capsule-old");
        std::fs::rename(dest, &backup_path)
            .context("failed to move existing .capsule/ to backup")?;
        if let Err(e) = std::fs::rename(&staging_capsule, dest) {
            let _ = std::fs::rename(&backup_path, dest);
            return Err(e).context("failed to move new .capsule/ into place");
        }
        drop(backup);
        drop(staging);
    } else {
        templates::copy_to(template, dest)?;
    }
    println!("Wrote .capsule/ from template \"{template}\".");
    println!();
    println!("Next:");
    println!("  - Read the agent guide:        capsule explain --all");
    println!("  - Customize prompts in:        .capsule/prompts/");
    println!("  - Validate after edits:        capsule check");
    println!(
        "  - Reference templates online:  https://github.com/DavKato/capsule/tree/main/templates"
    );
    Ok(())
}
