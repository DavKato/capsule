use std::path::Path;

use anyhow::Context;

use crate::config::{PipelineEntry, StageConfig};
use crate::verdict::{Verdict, VerdictStatus};

pub const SYSTEM_PREAMBLE: &str = include_str!("../../base-image/system_preamble.md");

pub(super) fn prepend_preamble(user_prompt: &str) -> String {
    format!("{SYSTEM_PREAMBLE}\n\n{user_prompt}")
}

pub(super) fn read_prompt_file(capsule_dir: &Path, path_str: &str) -> anyhow::Result<String> {
    let path = capsule_dir.join(path_str);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("prompt file not found: {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(super) fn resolve_all_prompts(
    entries: &mut [PipelineEntry],
    capsule_dir: Option<&Path>,
) -> anyhow::Result<()> {
    for entry in entries.iter_mut() {
        match entry {
            PipelineEntry::Stage(stage) => resolve_stage_prompt_inplace(stage, capsule_dir)?,
            PipelineEntry::Loop(loop_config) => {
                for stage in &mut loop_config.stages {
                    resolve_stage_prompt_inplace(stage, capsule_dir)?;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn resolve_stage_prompt_inplace(
    stage: &mut StageConfig,
    capsule_dir: Option<&Path>,
) -> anyhow::Result<()> {
    stage.prompt = match (stage.prompt.as_deref(), capsule_dir) {
        (Some(path_str), Some(dir)) => {
            let content = read_prompt_file(dir, path_str)
                .with_context(|| format!("stage '{}': failed to load prompt", stage.name))?;
            Some(prepend_preamble(&content))
        }
        (Some(literal), None) => Some(literal.to_string()),
        (None, _) => None,
    };
    Ok(())
}

pub(super) fn inject_input(input: &mut Option<String>, base_prompt: &str) -> String {
    if let Some(text) = input.take() {
        format!("<capsule:input>\n{text}\n</capsule:input>\n\n{base_prompt}")
    } else {
        base_prompt.to_string()
    }
}

pub(super) fn inject_note_block(
    last_stage: Option<&str>,
    last_verdict: &Option<Verdict>,
    base_prompt: &str,
) -> String {
    if let (Some(name), Some(verdict)) = (last_stage, last_verdict.as_ref()) {
        let notes = verdict.notes.as_deref().filter(|n| !n.is_empty());
        if let Some(notes) = notes {
            let status_str = match verdict.status {
                VerdictStatus::Pass => "pass",
                VerdictStatus::Fail => "fail",
                VerdictStatus::Done => "done",
            };
            let block = format!(
                "<previous-stage>\nStage: {name}\nStatus: {status_str}\nNotes: {notes}\n</previous-stage>"
            );
            return format!("{block}\n\n{base_prompt}");
        }
    }
    base_prompt.to_string()
}
