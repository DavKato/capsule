use anyhow::{anyhow, Result};
use include_dir::{include_dir, Dir};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

static TEMPLATES_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/templates");

pub struct TemplateEntry {
    pub name: String,
    pub description: String,
}

pub fn list() -> Vec<TemplateEntry> {
    let mut entries: Vec<TemplateEntry> = TEMPLATES_DIR
        .dirs()
        .filter_map(|dir| {
            let name = dir.path().file_name()?.to_str()?.to_owned();
            let desc_path = format!("{name}/description.txt");
            let description = dir
                .get_file(&desc_path)
                .and_then(|f| f.contents_utf8())
                .unwrap_or("")
                .trim()
                .to_owned();
            Some(TemplateEntry { name, description })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

pub fn copy_to(name: &str, dest: &Path) -> Result<()> {
    let template_dir = TEMPLATES_DIR
        .get_dir(name)
        .ok_or_else(|| anyhow!("unknown template: {name}"))?;

    let capsule_path = format!("{name}/.capsule");
    let capsule_subdir = template_dir
        .get_dir(&capsule_path)
        .ok_or_else(|| anyhow!("template {name} has no .capsule directory"))?;

    copy_dir(capsule_subdir, dest)
}

fn copy_dir(dir: &Dir, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let prefix = dir.path();

    for file in dir.files() {
        let relative = file.path().strip_prefix(prefix).unwrap();
        let target = dest.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, file.contents())?;
        #[cfg(unix)]
        if relative
            .extension()
            .is_some_and(|ext| ext == "sh" || ext == "bash")
        {
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
        }
    }

    for subdir in dir.dirs() {
        let relative = subdir.path().strip_prefix(prefix).unwrap();
        copy_dir(subdir, &dest.join(relative))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn list_includes_all_templates() {
        let names: HashSet<String> = list().into_iter().map(|e| e.name).collect();
        assert!(names.contains("ralph-loop"));
        assert!(names.contains("single-iter"));
    }

    #[test]
    fn list_entries_have_descriptions() {
        for entry in list() {
            assert!(
                !entry.description.is_empty(),
                "template {} has empty description",
                entry.name
            );
        }
    }

    #[test]
    fn copy_to_produces_capsule_files() {
        let dir = tempfile::tempdir().unwrap();
        copy_to("single-iter", dir.path()).unwrap();
        assert!(
            dir.path().join("config.yml").exists(),
            "config.yml missing after copy_to"
        );
        assert!(
            dir.path().join("Dockerfile").exists(),
            "Dockerfile missing after copy_to"
        );
    }

    #[test]
    fn copy_to_file_contents_are_identical() {
        let dir = tempfile::tempdir().unwrap();
        copy_to("single-iter", dir.path()).unwrap();
        let embedded = TEMPLATES_DIR
            .get_file("single-iter/.capsule/config.yml")
            .unwrap()
            .contents();
        let written = std::fs::read(dir.path().join("config.yml")).unwrap();
        assert_eq!(embedded, written.as_slice());
    }

    #[cfg(unix)]
    #[test]
    fn copy_to_sets_executable_on_shell_scripts() {
        let dir = tempfile::tempdir().unwrap();
        copy_to("single-iter", dir.path()).unwrap();
        for name in ["before-all.sh", "before-each.sh"] {
            let path = dir.path().join(name);
            if path.exists() {
                let mode = std::fs::metadata(&path).unwrap().permissions().mode();
                assert!(mode & 0o111 != 0, "{name} should be executable");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn copy_to_does_not_set_executable_on_non_scripts() {
        let dir = tempfile::tempdir().unwrap();
        copy_to("single-iter", dir.path()).unwrap();
        for name in ["Dockerfile", "config.yml", ".env"] {
            let path = dir.path().join(name);
            if path.exists() {
                let mode = std::fs::metadata(&path).unwrap().permissions().mode();
                assert!(mode & 0o111 == 0, "{name} should not be executable");
            }
        }
    }

    #[test]
    fn copy_to_unknown_template_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let result = copy_to("nonexistent", dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown template"));
    }
}
