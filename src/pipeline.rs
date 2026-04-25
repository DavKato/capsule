use std::collections::HashMap;

use crate::config::{OnFail, OnPass, PipelineConfig, PipelineEntry, StageConfig};
use crate::verdict::{Verdict, VerdictStatus};

pub trait StageRunner {
    fn run(&mut self, stage_name: &str, prompt: &str, model: Option<&str>) -> Option<Verdict>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum PipelineOutcome {
    /// All top-level stages completed, or a `done` verdict was emitted outside a loop.
    Done,
    /// An `exit` route was triggered.
    Exit,
    /// `max_pipeline_iterations` was exceeded.
    CapHit,
}

pub struct PipelineExecutor<R> {
    config: PipelineConfig,
    runner: R,
}

impl<R: StageRunner> PipelineExecutor<R> {
    pub fn new(config: PipelineConfig, runner: R) -> Self {
        Self { config, runner }
    }

    pub fn run(mut self) -> PipelineOutcome {
        let name_to_entry = build_name_index(&self.config);
        let max_pipeline = self.config.max_pipeline_iterations;
        let mut global_counter: u32 = 0;
        let mut fail_counts: HashMap<String, u32> = HashMap::new();
        let mut current_idx: usize = 0;

        loop {
            if current_idx >= self.config.entries.len() {
                return PipelineOutcome::Done;
            }

            if global_counter >= max_pipeline {
                return PipelineOutcome::CapHit;
            }
            global_counter += 1;

            let entry = self.config.entries[current_idx].clone();
            match entry {
                PipelineEntry::Stage(stage) => {
                    match run_stage(&mut self.runner, &stage, &name_to_entry, &mut fail_counts) {
                        StageOutcome::Advance(next_idx) => current_idx = next_idx,
                        StageOutcome::Done => return PipelineOutcome::Done,
                        StageOutcome::Exit => return PipelineOutcome::Exit,
                    }
                }
                PipelineEntry::Loop(_) => {
                    unimplemented!("loop entry handling is implemented in issue #61");
                }
            }
        }
    }
}

enum StageOutcome {
    Advance(usize),
    Done,
    Exit,
}

/// Builds a map from stage name to entry index for all `Stage` entries.
fn build_name_index(config: &PipelineConfig) -> HashMap<String, usize> {
    config
        .entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if let PipelineEntry::Stage(s) = e {
                Some((s.name.clone(), i))
            } else {
                None
            }
        })
        .collect()
}

fn run_stage(
    runner: &mut dyn StageRunner,
    stage: &StageConfig,
    name_to_entry: &HashMap<String, usize>,
    fail_counts: &mut HashMap<String, u32>,
) -> StageOutcome {
    let prompt = stage.prompt.as_deref().unwrap_or("");
    let verdict = runner.run(&stage.name, prompt, stage.model.as_deref());

    if matches!(
        verdict.as_ref().map(|v| &v.status),
        Some(VerdictStatus::Done)
    ) {
        return StageOutcome::Done;
    }

    let is_pass = matches!(
        verdict.as_ref().map(|v| &v.status),
        Some(VerdictStatus::Pass)
    );

    if is_pass {
        fail_counts.remove(&stage.name);
        route_pass(stage, name_to_entry)
    } else {
        let fail_count = fail_counts.entry(stage.name.clone()).or_insert(0);
        *fail_count += 1;

        if let Some(max) = stage.max_retries {
            if *fail_count > max {
                return StageOutcome::Exit;
            }
        }

        route_fail(stage, name_to_entry)
    }
}

fn route_pass(stage: &StageConfig, name_to_entry: &HashMap<String, usize>) -> StageOutcome {
    match &stage.on_pass {
        OnPass::Next => {
            let idx = name_to_entry.get(&stage.name).copied().unwrap_or(0);
            StageOutcome::Advance(idx + 1)
        }
        OnPass::Stage(name) => match name_to_entry.get(name.as_str()) {
            Some(&idx) => StageOutcome::Advance(idx),
            None => StageOutcome::Exit,
        },
        OnPass::Exit => StageOutcome::Exit,
    }
}

fn route_fail(stage: &StageConfig, name_to_entry: &HashMap<String, usize>) -> StageOutcome {
    match &stage.on_fail {
        OnFail::Exit => StageOutcome::Exit,
        OnFail::Retry => {
            let idx = name_to_entry.get(&stage.name).copied().unwrap_or(0);
            StageOutcome::Advance(idx)
        }
        OnFail::Stage(name) => match name_to_entry.get(name.as_str()) {
            Some(&idx) => StageOutcome::Advance(idx),
            None => StageOutcome::Exit,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OnFail, OnPass, PipelineConfig, PipelineEntry, StageConfig};
    use crate::verdict::{Verdict, VerdictStatus};
    use std::collections::VecDeque;

    struct FakeRunner {
        responses: VecDeque<Option<Verdict>>,
    }

    impl FakeRunner {
        fn new(responses: impl IntoIterator<Item = Option<Verdict>>) -> Self {
            Self {
                responses: responses.into_iter().collect(),
            }
        }
    }

    impl StageRunner for FakeRunner {
        fn run(
            &mut self,
            _stage_name: &str,
            _prompt: &str,
            _model: Option<&str>,
        ) -> Option<Verdict> {
            self.responses
                .pop_front()
                .expect("FakeRunner: no more responses queued")
        }
    }

    fn pass() -> Option<Verdict> {
        Some(Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        })
    }

    fn fail() -> Option<Verdict> {
        Some(Verdict {
            status: VerdictStatus::Fail,
            notes: None,
        })
    }

    fn done() -> Option<Verdict> {
        Some(Verdict {
            status: VerdictStatus::Done,
            notes: None,
        })
    }

    fn stage(name: &str) -> StageConfig {
        StageConfig {
            name: name.to_string(),
            prompt: None,
            model: None,
            on_pass: OnPass::Next,
            on_fail: OnFail::Exit,
            max_retries: None,
        }
    }

    fn pipeline(entries: Vec<PipelineEntry>) -> PipelineConfig {
        PipelineConfig {
            entries,
            max_pipeline_iterations: 1000,
        }
    }

    fn single_stage_entry(s: StageConfig) -> PipelineEntry {
        PipelineEntry::Stage(s)
    }

    // Linear three-stage happy path: all pass, pipeline reaches Done.
    #[test]
    fn linear_three_stage_all_pass() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
            single_stage_entry(stage("c")),
        ]);
        let runner = FakeRunner::new([pass(), pass(), pass()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::Done
        );
    }

    // on_fail: exit (default) terminates pipeline on first fail.
    #[test]
    fn on_fail_exit_default_terminates_on_first_fail() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
        ]);
        let runner = FakeRunner::new([fail()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::Exit
        );
    }

    // on_fail: retry — stage retries until pass.
    #[test]
    fn on_fail_retry_retries_until_pass() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        let config = pipeline(vec![single_stage_entry(s)]);
        let runner = FakeRunner::new([fail(), fail(), pass()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::Done
        );
    }

    // on_fail: retry — max_retries exceeded causes exit.
    #[test]
    fn on_fail_retry_exits_when_max_retries_exceeded() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = Some(2);
        let config = pipeline(vec![single_stage_entry(s)]);
        // 3 fails: fail_count reaches 3 > 2, exit
        let runner = FakeRunner::new([fail(), fail(), fail()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::Exit
        );
    }

    // on_fail: <stage> — loops back and resumes forward progress.
    #[test]
    fn on_fail_stage_loops_back_and_resumes() {
        let a = stage("a");
        let mut b = stage("b");
        b.on_fail = OnFail::Stage("a".to_string());
        let c = stage("c");

        let config = pipeline(vec![
            single_stage_entry(a),
            single_stage_entry(b),
            single_stage_entry(c),
        ]);
        // a passes, b fails (jump to a), a passes, b passes, c passes
        let runner = FakeRunner::new([pass(), fail(), pass(), pass(), pass()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::Done
        );
    }

    // max_retries counts stage-specific fails and resets on pass.
    #[test]
    fn max_retries_resets_on_pass() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = Some(2);
        let config = pipeline(vec![single_stage_entry(s)]);
        // fail, fail, pass (reset), fail, fail, pass — all within max_retries
        let runner = FakeRunner::new([fail(), fail(), pass(), fail(), fail(), pass()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::Done
        );
    }

    // max_pipeline_iterations caps total invocations regardless of per-stage counters.
    #[test]
    fn max_pipeline_iterations_caps_total() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_pipeline_iterations: 3,
        };
        // 4 fails: 3rd triggers cap, 4th never runs
        let runner = FakeRunner::new([fail(), fail(), fail(), fail()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::CapHit
        );
    }

    // Silent exit (no verdict) is treated as implicit fail and routes via on_fail.
    #[test]
    fn silent_exit_treated_as_implicit_fail() {
        let config = pipeline(vec![single_stage_entry(stage("a"))]);
        // None → implicit fail → on_fail: exit → Exit
        let runner = FakeRunner::new([None]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::Exit
        );
    }

    // done verdict outside a loop terminates pipeline with Done.
    #[test]
    fn done_verdict_terminates_pipeline_done() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
        ]);
        // a emits done → pipeline Done immediately (b never runs)
        let runner = FakeRunner::new([done()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::Done
        );
    }
}
