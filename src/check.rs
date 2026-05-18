use crate::config::{Config, PipelineConfig, PipelineEntry};
use std::path::Path;

#[derive(Debug, PartialEq)]
pub enum Severity {
    Error,
    Hint,
}

#[derive(Debug)]
pub struct CheckIssue {
    pub severity: Severity,
    pub location: String,
    pub message: String,
    pub fix_hint: Option<String>,
}

pub type CheckReport = Vec<CheckIssue>;

pub fn check(cfg: &Config) -> CheckReport {
    let mut issues = Vec::new();
    check_dockerfile(&cfg.capsule_dir, &mut issues);
    check_prompt_files(&cfg.pipeline, &cfg.capsule_dir, &mut issues);
    check_hook_scripts(cfg, &mut issues);
    issues
}

fn check_dockerfile(capsule_dir: &Path, issues: &mut CheckReport) {
    if !capsule_dir.join("Dockerfile").exists() {
        issues.push(CheckIssue {
            severity: Severity::Hint,
            location: "Dockerfile".to_string(),
            message: "no Dockerfile — the base capsule image will be used as-is".to_string(),
            fix_hint: Some("add a Dockerfile to install extra dependencies".to_string()),
        });
    }
}

fn check_prompt_files(pipeline: &PipelineConfig, capsule_dir: &Path, issues: &mut CheckReport) {
    for stage in all_stages(pipeline) {
        if let Some(ref prompt_path) = stage.prompt {
            let full_path = capsule_dir.join(prompt_path);
            if !full_path.exists() {
                issues.push(CheckIssue {
                    severity: Severity::Error,
                    location: format!("stages.{}", stage.name),
                    message: format!("prompt file not found: {prompt_path}"),
                    fix_hint: Some(format!("create the file at {}", full_path.display())),
                });
            }
        }
    }
}

fn check_hook_scripts(cfg: &Config, issues: &mut CheckReport) {
    for name in ["before-all.sh", "before-each.sh"] {
        let path = cfg.capsule_dir.join(name);
        if path.exists() {
            issues.push(CheckIssue {
                severity: Severity::Error,
                location: name.to_string(),
                message: format!(
                    "{name} is no longer used — migrate to the `setup` field in config.yml"
                ),
                fix_hint: Some(
                    "add `setup: <command-or-script>` at the top level or per stage".to_string(),
                ),
            });
        }
    }

    if let Some(ref value) = cfg.setup {
        check_setup_value(value, "setup", &cfg.capsule_dir, issues);
    }
    for stage in all_stages(&cfg.pipeline) {
        if let Some(ref value) = stage.setup {
            let location = format!("stages.{}.setup", stage.name);
            check_setup_value(value, &location, &cfg.capsule_dir, issues);
        }
    }
}

fn check_setup_value(value: &str, location: &str, capsule_dir: &Path, issues: &mut CheckReport) {
    if value.contains(char::is_whitespace) {
        return;
    }

    let path = capsule_dir.join(value);
    if !path.exists() {
        issues.push(CheckIssue {
            severity: Severity::Error,
            location: location.to_string(),
            message: format!("setup file not found: {value}"),
            fix_hint: Some(format!("create the file at {}", path.display())),
        });
        return;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.permissions().mode() & 0o111 == 0 {
                issues.push(CheckIssue {
                    severity: Severity::Hint,
                    location: location.to_string(),
                    message: format!("{value} is not executable"),
                    fix_hint: Some(format!("chmod +x {}", path.display())),
                });
            }
        }
    }
}

fn all_stages(pipeline: &PipelineConfig) -> Vec<&crate::config::StageConfig> {
    let mut stages = Vec::new();
    for entry in &pipeline.entries {
        match entry {
            PipelineEntry::Stage(s) => stages.push(s),
            PipelineEntry::Loop(l) => {
                for s in &l.stages {
                    stages.push(s);
                }
            }
        }
    }
    stages
}
