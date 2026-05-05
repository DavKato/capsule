use anyhow::{bail, Result};
use std::path::Path;

use crate::templates;

pub fn init(template: &str, dest: &Path, force: bool) -> Result<()> {
    if dest.exists() {
        if !force {
            bail!(".capsule/ already exists. Use --force to overwrite existing setup.");
        }
        std::fs::remove_dir_all(dest)?;
    }
    templates::copy_to(template, dest)?;
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
