use crate::config::PipelineConfig;
use crate::config::PipelineEntry;
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

pub fn check(pipeline: &PipelineConfig, capsule_dir: &Path) -> CheckReport {
    let mut issues = Vec::new();
    check_dockerfile(capsule_dir, &mut issues);
    check_prompt_files(pipeline, capsule_dir, &mut issues);
    check_hook_scripts(capsule_dir, &mut issues);
    issues
}

fn check_dockerfile(capsule_dir: &Path, issues: &mut CheckReport) {
    if !capsule_dir.join("Dockerfile").exists() {
        issues.push(CheckIssue {
            severity: Severity::Error,
            location: "Dockerfile".to_string(),
            message: "Dockerfile not found in capsule directory".to_string(),
            fix_hint: Some("run `capsule init` to scaffold a Dockerfile".to_string()),
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

fn check_hook_scripts(capsule_dir: &Path, issues: &mut CheckReport) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["before-all.sh", "before-each.sh"] {
            let path = capsule_dir.join(name);
            if path.exists() {
                if let Ok(meta) = std::fs::metadata(&path) {
                    if meta.permissions().mode() & 0o111 == 0 {
                        issues.push(CheckIssue {
                            severity: Severity::Hint,
                            location: name.to_string(),
                            message: format!("{name} is not executable"),
                            fix_hint: Some(format!("chmod +x {}", path.display())),
                        });
                    }
                }
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
