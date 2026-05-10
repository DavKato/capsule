use capsule::config::{LoopConfig, OnFail, OnPass, PipelineConfig, PipelineEntry, StageConfig};
use capsule::pipeline::{PipelineExecutor, StageRunner, SYSTEM_PREAMBLE};
use capsule::verdict::{Verdict, VerdictStatus};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct RecordingRunner {
    responses: VecDeque<Option<Verdict>>,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl RecordingRunner {
    fn new(
        responses: impl IntoIterator<Item = Option<Verdict>>,
    ) -> (Self, Arc<Mutex<Vec<String>>>) {
        let prompts = Arc::new(Mutex::new(vec![]));
        (
            Self {
                responses: responses.into_iter().collect(),
                prompts: Arc::clone(&prompts),
            },
            prompts,
        )
    }
}

impl StageRunner for RecordingRunner {
    fn run(
        &mut self,
        _stage_name: &str,
        prompt: &str,
        _model: Option<&str>,
        _retry: Option<&capsule::pipeline::RetryInfo>,
    ) -> anyhow::Result<Option<Verdict>> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        Ok(self.responses.pop_front().expect("no more responses"))
    }
}

fn pass() -> Option<Verdict> {
    Some(Verdict {
        status: VerdictStatus::Pass,
        notes: None,
    })
}

fn single_stage_config(prompt: Option<&str>) -> PipelineConfig {
    PipelineConfig {
        entries: vec![PipelineEntry::Stage(StageConfig {
            name: "s".to_string(),
            prompt: prompt.map(str::to_string),
            model: None,
            on_pass: OnPass::Next,
            on_fail: OnFail::Exit,
            max_retries: None,
        })],
        max_pipeline_iterations: 10,
        cap_hit_is_ok: false,
    }
}

fn loop_stage_config(prompt: Option<&str>) -> PipelineConfig {
    PipelineConfig {
        entries: vec![PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(1),
            stages: vec![StageConfig {
                name: "s".to_string(),
                prompt: prompt.map(str::to_string),
                model: None,
                on_pass: OnPass::Next,
                on_fail: OnFail::Exit,
                max_retries: None,
            }],
        })],
        max_pipeline_iterations: 10,
        cap_hit_is_ok: true,
    }
}

#[test]
fn system_preamble_is_non_empty() {
    assert!(!SYSTEM_PREAMBLE.trim().is_empty());
}

#[test]
fn executor_reads_prompt_file_and_prepends_preamble() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("prompt.md"), "do the thing").unwrap();
    let (runner, prompts) = RecordingRunner::new([pass()]);
    PipelineExecutor::new(single_stage_config(Some("prompt.md")), runner)
        .with_capsule_dir(dir.path().to_path_buf())
        .run()
        .unwrap();
    let prompts = prompts.lock().unwrap();
    let prompt = &prompts[0];
    assert!(prompt.contains(SYSTEM_PREAMBLE), "preamble must be present");
    assert!(
        prompt.contains("do the thing"),
        "file content must be present"
    );
    let preamble_pos = prompt.find(SYSTEM_PREAMBLE).unwrap();
    let content_pos = prompt.find("do the thing").unwrap();
    assert!(
        preamble_pos < content_pos,
        "preamble must precede user content"
    );
}

#[test]
fn loop_stage_executor_reads_prompt_file_and_prepends_preamble() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("loop_prompt.md"), "loop task").unwrap();
    let (runner, prompts) = RecordingRunner::new([pass()]);
    PipelineExecutor::new(loop_stage_config(Some("loop_prompt.md")), runner)
        .with_capsule_dir(dir.path().to_path_buf())
        .run()
        .unwrap();
    let prompts = prompts.lock().unwrap();
    let prompt = &prompts[0];
    assert!(prompt.contains(SYSTEM_PREAMBLE));
    assert!(prompt.contains("loop task"));
}

#[test]
fn without_capsule_dir_stage_prompt_used_as_literal() {
    let (runner, prompts) = RecordingRunner::new([pass()]);
    PipelineExecutor::new(single_stage_config(Some("literal content")), runner)
        .run()
        .unwrap();
    assert_eq!(prompts.lock().unwrap()[0], "literal content");
}

#[test]
fn missing_prompt_file_returns_error_with_path() {
    let dir = tempfile::tempdir().unwrap();
    let (runner, _) = RecordingRunner::new([pass()]);
    let result = PipelineExecutor::new(single_stage_config(Some("nonexistent.md")), runner)
        .with_capsule_dir(dir.path().to_path_buf())
        .run();
    let err = result.unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("nonexistent.md"),
        "error should name the missing file; got: {msg}"
    );
}

#[test]
fn explicit_absolute_path_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(other.path(), "from explicit path").unwrap();
    let path_str = other.path().to_str().unwrap().to_string();
    let (runner, prompts) = RecordingRunner::new([pass()]);
    PipelineExecutor::new(single_stage_config(Some(&path_str)), runner)
        .with_capsule_dir(dir.path().to_path_buf())
        .run()
        .unwrap();
    assert!(prompts.lock().unwrap()[0].contains("from explicit path"));
}
