use std::collections::HashMap;

use crate::config::{LoopConfig, OnFail, OnPass, PipelineConfig, PipelineEntry, StageConfig};
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

            let entry = self.config.entries[current_idx].clone();
            match entry {
                PipelineEntry::Stage(stage) => {
                    if global_counter >= max_pipeline {
                        return PipelineOutcome::CapHit;
                    }
                    global_counter += 1;
                    match run_stage(&mut self.runner, &stage, &name_to_entry, &mut fail_counts) {
                        StageOutcome::Advance(next_idx) => current_idx = next_idx,
                        StageOutcome::Done => return PipelineOutcome::Done,
                        StageOutcome::Exit => return PipelineOutcome::Exit,
                    }
                }
                PipelineEntry::Loop(loop_config) => {
                    match run_loop(
                        &mut self.runner,
                        &loop_config,
                        &mut fail_counts,
                        &mut global_counter,
                        max_pipeline,
                    ) {
                        LoopOutcome::LoopDone => current_idx += 1,
                        LoopOutcome::Exit => return PipelineOutcome::Exit,
                        LoopOutcome::CapHit => return PipelineOutcome::CapHit,
                    }
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

enum LoopOutcome {
    LoopDone,
    Exit,
    CapHit,
}

fn run_loop(
    runner: &mut dyn StageRunner,
    loop_config: &LoopConfig,
    fail_counts: &mut HashMap<String, u32>,
    global_counter: &mut u32,
    max_pipeline: u32,
) -> LoopOutcome {
    let loop_name_to_idx: HashMap<String, usize> = loop_config
        .stages
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.clone(), i))
        .collect();

    let mut iteration_count: u32 = 0;
    let mut stage_idx: usize = 0;
    let mut retrying_top = false;

    loop {
        if stage_idx >= loop_config.stages.len() {
            stage_idx = 0;
            retrying_top = false;
        }

        if stage_idx == 0 && !retrying_top {
            iteration_count += 1;
            if let Some(max) = loop_config.max_iteration {
                if iteration_count > max {
                    return LoopOutcome::CapHit;
                }
            }
        }
        retrying_top = false;

        if *global_counter >= max_pipeline {
            return LoopOutcome::CapHit;
        }
        *global_counter += 1;

        let stage = loop_config.stages[stage_idx].clone();
        let prompt = stage.prompt.as_deref().unwrap_or("");
        let verdict = runner.run(&stage.name, prompt, stage.model.as_deref());

        if matches!(
            verdict.as_ref().map(|v| &v.status),
            Some(VerdictStatus::Done)
        ) {
            return LoopOutcome::LoopDone;
        }

        let is_pass = matches!(
            verdict.as_ref().map(|v| &v.status),
            Some(VerdictStatus::Pass)
        );

        if is_pass {
            fail_counts.remove(&stage.name);
            match &stage.on_pass {
                OnPass::Next => stage_idx += 1,
                OnPass::Stage(name) => match loop_name_to_idx.get(name.as_str()) {
                    Some(&idx) => stage_idx = idx,
                    None => return LoopOutcome::Exit,
                },
                OnPass::Exit => return LoopOutcome::Exit,
            }
        } else {
            let fail_count = fail_counts.entry(stage.name.clone()).or_insert(0);
            *fail_count += 1;

            if let Some(max) = stage.max_retries {
                if *fail_count > max {
                    return LoopOutcome::Exit;
                }
            }

            match &stage.on_fail {
                OnFail::Exit => return LoopOutcome::Exit,
                OnFail::Retry => {
                    if stage_idx == 0 {
                        retrying_top = true;
                    }
                }
                OnFail::Stage(name) => match loop_name_to_idx.get(name.as_str()) {
                    Some(&idx) => stage_idx = idx,
                    None => return LoopOutcome::Exit,
                },
            }
        }
    }
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
    use crate::config::{LoopConfig, OnFail, OnPass, PipelineConfig, PipelineEntry, StageConfig};
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

    // done inside a loop exits the loop; pipeline continues with post-loop stages.
    #[test]
    fn done_inside_loop_exits_loop_pipeline_continues() {
        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(10),
            stages: vec![stage("a")],
        });
        let config = pipeline(vec![loop_entry, single_stage_entry(stage("b"))]);
        // a emits done → loop exits → b runs and passes → pipeline Done
        let runner = FakeRunner::new([done(), pass()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::Done
        );
    }

    // max_iteration cap-hit terminates pipeline non-zero.
    #[test]
    fn max_iteration_cap_hit_terminates_nok() {
        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(2),
            stages: vec![stage("a")],
        });
        let config = pipeline(vec![loop_entry]);
        // a passes twice (iterations 1 and 2), then iteration 3 exceeds cap → CapHit
        let runner = FakeRunner::new([pass(), pass()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::CapHit
        );
    }

    // max_iteration ticks on top-of-body re-entry via explicit route, not on self-retry.
    #[test]
    fn max_iteration_ticks_on_top_of_body_reentry() {
        let implementer = stage("implementer");
        let mut reviewer = stage("reviewer");
        reviewer.on_fail = OnFail::Stage("implementer".to_string());

        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(2),
            stages: vec![implementer, reviewer],
        });
        let config = pipeline(vec![loop_entry]);
        // iteration 1: implementer pass, reviewer fail → back to implementer (ticks to 2)
        // iteration 2: implementer pass, reviewer pass → end of body → iteration 3 > 2 → CapHit
        let runner = FakeRunner::new([pass(), fail(), pass(), pass()]);
        assert_eq!(
            PipelineExecutor::new(config, runner).run(),
            PipelineOutcome::CapHit
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
