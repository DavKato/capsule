mod prompt;
mod routing;
mod state;
mod summary;

use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::{PipelineConfig, PipelineEntry};

pub use prompt::SYSTEM_PREAMBLE;
pub use state::PipelineState;
pub use summary::{
    build_summary_artifact, CapHitKind, FailExitKind, IterationCounters, PipelineOutcome,
    RunSummary, TerminalReason,
};

use routing::{
    build_name_index, handle_loop_outcome, run_loop, run_stage, ExitKind, LoopControl,
    PipelineProgress, StageOutcome,
};
use summary::CapHitKind as CapHit;

/// Info about the current retry attempt — shown only when retrying.
pub struct RetryInfo {
    /// How many times the stage has already failed (1 = first retry).
    pub current: u32,
    pub max: u32,
}

pub trait StageRunner {
    fn run(
        &mut self,
        stage_name: &str,
        prompt: &str,
        model: Option<&str>,
        setup: Option<&str>,
        retry: Option<&RetryInfo>,
    ) -> anyhow::Result<Option<crate::verdict::Verdict>>;
}

/// Return value of `PipelineExecutor::run`.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineRunResult {
    pub outcome: PipelineOutcome,
    pub summary: RunSummary,
    pub pipeline_state: PipelineState,
}

pub struct PipelineExecutor<R> {
    config: PipelineConfig,
    runner: R,
    input: Option<String>,
    initial_state: Option<PipelineState>,
    capsule_dir: Option<PathBuf>,
}

impl<R: StageRunner> PipelineExecutor<R> {
    pub fn new(config: PipelineConfig, runner: R) -> Self {
        Self {
            config,
            runner,
            input: None,
            initial_state: None,
            capsule_dir: None,
        }
    }

    pub fn resume(config: PipelineConfig, runner: R, state: PipelineState) -> Self {
        Self {
            config,
            runner,
            input: None,
            initial_state: Some(state),
            capsule_dir: None,
        }
    }

    pub fn with_input(mut self, input: Option<String>) -> Self {
        self.input = input;
        self
    }

    pub fn with_capsule_dir(mut self, dir: PathBuf) -> Self {
        self.capsule_dir = Some(dir);
        self
    }

    pub fn run(mut self) -> anyhow::Result<(PipelineRunResult, R)> {
        prompt::resolve_all_prompts(&mut self.config.entries, self.capsule_dir.as_deref())?;

        let name_to_entry = build_name_index(&self.config);
        let max_pipeline = self.config.max_stages;

        let (mut current_idx, mut loop_iterations, mut progress) = match self.initial_state.take() {
            Some(s) => (
                s.current_idx,
                s.loop_iterations,
                PipelineProgress {
                    retry_counts: s.retry_counts,
                    failure_totals: s.failure_totals,
                    global_counter: s.global_counter,
                    input: self.input.take(),
                    last_stage: s.last_stage,
                    last_verdict: s.last_verdict,
                    fail_exit_info: None,
                },
            ),
            None => (
                0,
                HashMap::new(),
                PipelineProgress {
                    retry_counts: HashMap::new(),
                    failure_totals: HashMap::new(),
                    global_counter: 0,
                    input: self.input.take(),
                    last_stage: None,
                    last_verdict: None,
                    fail_exit_info: None,
                },
            ),
        };

        let (outcome, cap_hit) = 'pipeline: loop {
            if current_idx >= self.config.entries.len() {
                break (PipelineOutcome::Done, None);
            }

            match &self.config.entries[current_idx] {
                PipelineEntry::Stage(stage) => {
                    if progress.global_counter >= max_pipeline {
                        break (
                            PipelineOutcome::CapHit,
                            Some(CapHit::MaxStages {
                                limit: max_pipeline,
                            }),
                        );
                    }
                    progress.global_counter += 1;
                    match run_stage(&mut self.runner, stage, &name_to_entry, &mut progress)? {
                        StageOutcome::Advance(next_idx) => current_idx = next_idx,
                        StageOutcome::AdvanceIntoLoop {
                            entry_idx,
                            stage_idx,
                        } => {
                            let PipelineEntry::Loop(loop_config) = &self.config.entries[entry_idx]
                            else {
                                unreachable!("AdvanceIntoLoop references non-loop entry");
                            };
                            match handle_loop_outcome(
                                run_loop(
                                    &mut self.runner,
                                    loop_config,
                                    &mut progress,
                                    max_pipeline,
                                    stage_idx,
                                )?,
                                entry_idx,
                                &mut loop_iterations,
                            ) {
                                LoopControl::Advance(next) => current_idx = next,
                                LoopControl::Break(o, cap) => break 'pipeline (o, cap),
                            }
                        }
                        StageOutcome::Done => break (PipelineOutcome::Done, None),
                        StageOutcome::Exit(ExitKind::PassRoute) => {
                            break (PipelineOutcome::Exit { from_fail: false }, None)
                        }
                        StageOutcome::Exit(ExitKind::FailRoute) => {
                            break (PipelineOutcome::Exit { from_fail: true }, None)
                        }
                    }
                }
                PipelineEntry::Loop(loop_config) => {
                    let entry_idx = current_idx;
                    match handle_loop_outcome(
                        run_loop(
                            &mut self.runner,
                            loop_config,
                            &mut progress,
                            max_pipeline,
                            0,
                        )?,
                        entry_idx,
                        &mut loop_iterations,
                    ) {
                        LoopControl::Advance(next) => current_idx = next,
                        LoopControl::Break(o, cap) => break (o, cap),
                    }
                }
            }
        };

        let terminal_reason = match &outcome {
            PipelineOutcome::Done => TerminalReason::Done,
            PipelineOutcome::Exit { from_fail: false } => TerminalReason::Exit,
            PipelineOutcome::Exit { from_fail: true } => {
                let (stage, kind) = progress.fail_exit_info.take().unwrap_or_else(|| {
                    (
                        progress.last_stage.clone().unwrap_or_default(),
                        FailExitKind::Route,
                    )
                });
                TerminalReason::FailExit { stage, kind }
            }
            PipelineOutcome::CapHit => TerminalReason::CapHit,
        };

        let pipeline_state = PipelineState {
            current_idx,
            global_counter: progress.global_counter,
            retry_counts: progress.retry_counts,
            failure_totals: progress.failure_totals,
            last_stage: progress.last_stage.clone(),
            last_verdict: progress.last_verdict.clone(),
            loop_iterations: loop_iterations.clone(),
            env: vec![],
        };

        Ok((
            PipelineRunResult {
                outcome,
                summary: RunSummary {
                    terminal_reason,
                    last_stage: progress.last_stage,
                    last_verdict: progress.last_verdict,
                    iteration_counters: IterationCounters {
                        global: pipeline_state.global_counter,
                        loops: loop_iterations,
                    },
                    cap_hit,
                    session_id: None,
                },
                pipeline_state,
            },
            self.runner,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        LoopConfig, OnFail, OnPass, PipelineConfig, PipelineEntry, StageConfig, MAX_RETRIES_DEFAULT,
    };
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
            _setup: Option<&str>,
            _retry: Option<&RetryInfo>,
        ) -> anyhow::Result<Option<Verdict>> {
            Ok(self
                .responses
                .pop_front()
                .expect("FakeRunner: no more responses queued"))
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
            max_retries: MAX_RETRIES_DEFAULT,
            max_failure: None,
            setup: None,
        }
    }

    fn pipeline(entries: Vec<PipelineEntry>) -> PipelineConfig {
        PipelineConfig {
            entries,
            max_stages: 1000,
        }
    }

    fn single_stage_entry(s: StageConfig) -> PipelineEntry {
        PipelineEntry::Stage(s)
    }

    fn run_outcome(config: PipelineConfig, runner: FakeRunner) -> PipelineOutcome {
        PipelineExecutor::new(config, runner)
            .run()
            .unwrap()
            .0
            .outcome
    }

    #[test]
    fn linear_three_stage_all_pass() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
            single_stage_entry(stage("c")),
        ]);
        assert_eq!(
            run_outcome(config, FakeRunner::new([pass(), pass(), pass()])),
            PipelineOutcome::Done
        );
    }

    #[test]
    fn on_fail_exit_default_terminates_on_first_fail() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
        ]);
        assert_eq!(
            run_outcome(config, FakeRunner::new([fail()])),
            PipelineOutcome::Exit { from_fail: true }
        );
    }

    #[test]
    fn on_fail_retry_retries_until_pass() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        let config = pipeline(vec![single_stage_entry(s)]);
        assert_eq!(
            run_outcome(config, FakeRunner::new([fail(), fail(), pass()])),
            PipelineOutcome::Done
        );
    }

    #[test]
    fn on_fail_retry_exits_when_max_retries_exceeded() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = 2;
        let config = pipeline(vec![single_stage_entry(s)]);
        // 3 fails: fail_count reaches 3 > 2, exit
        assert_eq!(
            run_outcome(config, FakeRunner::new([fail(), fail(), fail()])),
            PipelineOutcome::Exit { from_fail: true }
        );
    }

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
        assert_eq!(
            run_outcome(
                config,
                FakeRunner::new([pass(), fail(), pass(), pass(), pass()])
            ),
            PipelineOutcome::Done
        );
    }

    #[test]
    fn max_retries_resets_on_pass() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = 2;
        let config = pipeline(vec![single_stage_entry(s)]);
        // fail, fail, pass (reset), fail, fail, pass — all within max_retries
        assert_eq!(
            run_outcome(
                config,
                FakeRunner::new([fail(), fail(), pass(), fail(), fail(), pass()])
            ),
            PipelineOutcome::Done
        );
    }

    #[test]
    fn max_stages_caps_total() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_stages: 3,
        };
        // 4 fails: 3rd triggers cap, 4th never runs
        assert_eq!(
            run_outcome(config, FakeRunner::new([fail(), fail(), fail(), fail()])),
            PipelineOutcome::CapHit
        );
    }

    #[test]
    fn silent_exit_treated_as_implicit_fail() {
        let config = pipeline(vec![single_stage_entry(stage("a"))]);
        // None → implicit fail → on_fail: exit → Exit { from_fail: true }
        assert_eq!(
            run_outcome(config, FakeRunner::new([None])),
            PipelineOutcome::Exit { from_fail: true }
        );
    }

    #[test]
    fn done_inside_loop_exits_loop_pipeline_continues() {
        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(10),
            stages: vec![stage("a")],
        });
        let config = pipeline(vec![loop_entry, single_stage_entry(stage("b"))]);
        // a emits done → loop exits → b runs and passes → pipeline Done
        assert_eq!(
            run_outcome(config, FakeRunner::new([done(), pass()])),
            PipelineOutcome::Done
        );
    }

    #[test]
    fn max_iteration_cap_hit_terminates_nok() {
        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(2),
            stages: vec![stage("a")],
        });
        let config = pipeline(vec![loop_entry]);
        // a passes twice (iterations 1 and 2), then iteration 3 exceeds cap → CapHit
        assert_eq!(
            run_outcome(config, FakeRunner::new([pass(), pass()])),
            PipelineOutcome::CapHit
        );
    }

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
        assert_eq!(
            run_outcome(config, FakeRunner::new([pass(), fail(), pass(), pass()])),
            PipelineOutcome::CapHit
        );
    }

    #[test]
    fn done_verdict_terminates_pipeline_done() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
        ]);
        // a emits done → pipeline Done immediately (b never runs)
        let runner = FakeRunner::new([done()]);
        assert_eq!(
            PipelineExecutor::new(config, runner)
                .run()
                .unwrap()
                .0
                .outcome,
            PipelineOutcome::Done
        );
    }

    use std::sync::{Arc, Mutex};

    struct RecordingRunner {
        responses: VecDeque<Option<Verdict>>,
        prompts: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingRunner {
        fn new(
            responses: impl IntoIterator<Item = Option<Verdict>>,
        ) -> (Self, Arc<Mutex<Vec<String>>>) {
            let prompts = Arc::new(Mutex::new(Vec::new()));
            let runner = Self {
                responses: responses.into_iter().collect(),
                prompts: Arc::clone(&prompts),
            };
            (runner, prompts)
        }
    }

    impl StageRunner for RecordingRunner {
        fn run(
            &mut self,
            _stage_name: &str,
            prompt: &str,
            _model: Option<&str>,
            _setup: Option<&str>,
            _retry: Option<&RetryInfo>,
        ) -> anyhow::Result<Option<Verdict>> {
            self.prompts.lock().unwrap().push(prompt.to_string());
            Ok(self
                .responses
                .pop_front()
                .expect("RecordingRunner: no more responses queued"))
        }
    }

    #[test]
    fn input_injected_on_first_invocation_only() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
        ]);
        let (runner, prompts) = RecordingRunner::new([pass(), pass()]);
        PipelineExecutor::new(config, runner)
            .with_input(Some("my-input".to_string()))
            .run()
            .unwrap();
        let prompts = prompts.lock().unwrap();
        assert!(
            prompts[0].contains("<capsule:input>"),
            "first prompt should contain input block"
        );
        assert!(
            prompts[0].contains("my-input"),
            "first prompt should contain input text"
        );
        assert!(
            !prompts[1].contains("<capsule:input>"),
            "second prompt must not contain input block"
        );
    }

    #[test]
    fn no_input_prompts_unchanged() {
        let mut s = stage("a");
        s.prompt = Some("hello".to_string());
        let config = pipeline(vec![single_stage_entry(s)]);
        let (runner, prompts) = RecordingRunner::new([pass()]);
        PipelineExecutor::new(config, runner).run().unwrap();
        assert_eq!(prompts.lock().unwrap()[0], "hello");
    }

    fn pass_with_notes(notes: &str) -> Option<Verdict> {
        Some(Verdict {
            status: VerdictStatus::Pass,
            notes: Some(notes.to_string()),
        })
    }

    #[test]
    fn first_stage_has_no_note_block() {
        let config = pipeline(vec![single_stage_entry(stage("a"))]);
        let (runner, prompts) = RecordingRunner::new([pass_with_notes("first done")]);
        PipelineExecutor::new(config, runner).run().unwrap();
        assert!(!prompts.lock().unwrap()[0].contains("<previous-stage>"));
    }

    #[test]
    fn second_stage_receives_note_block_from_first() {
        let mut a = stage("a");
        a.prompt = Some("prompt-a".to_string());
        let mut b = stage("b");
        b.prompt = Some("prompt-b".to_string());
        let config = pipeline(vec![single_stage_entry(a), single_stage_entry(b)]);
        let (runner, prompts) = RecordingRunner::new([pass_with_notes("result from a"), pass()]);
        PipelineExecutor::new(config, runner).run().unwrap();
        let prompts = prompts.lock().unwrap();
        assert!(
            !prompts[0].contains("<previous-stage>"),
            "first prompt must not have note block"
        );
        assert!(
            prompts[1].contains("<previous-stage>"),
            "second prompt must have note block"
        );
        assert!(
            prompts[1].contains("Stage: a"),
            "note block must reference previous stage name"
        );
        assert!(prompts[1].contains("Status: pass"));
        assert!(prompts[1].contains("Notes: result from a"));
        assert!(prompts[1].contains("prompt-b"), "base prompt still present");
    }

    #[test]
    fn no_note_block_when_previous_verdict_has_no_notes() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
        ]);
        let (runner, prompts) = RecordingRunner::new([pass(), pass()]);
        PipelineExecutor::new(config, runner).run().unwrap();
        assert!(!prompts.lock().unwrap()[1].contains("<previous-stage>"));
    }

    #[test]
    fn loop_note_block_carried_between_iterations() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(2),
            stages: vec![s],
        });
        let config = pipeline(vec![loop_entry]);
        // iteration 1 passes with notes, iteration 2 passes (cap then terminates)
        let (runner, prompts) = RecordingRunner::new([pass_with_notes("iter 1 output"), pass()]);
        PipelineExecutor::new(config, runner).run().unwrap();
        let prompts = prompts.lock().unwrap();
        assert!(
            !prompts[0].contains("<previous-stage>"),
            "first call must not have note block"
        );
        assert!(
            prompts[1].contains("<previous-stage>"),
            "second call must have note block"
        );
        assert!(prompts[1].contains("iter 1 output"));
    }

    #[test]
    fn note_block_ordering_notes_before_input_before_base() {
        let mut a = stage("a");
        a.prompt = Some("task-a".to_string());
        let mut b = stage("b");
        b.prompt = Some("task-b".to_string());
        let config = pipeline(vec![single_stage_entry(a), single_stage_entry(b)]);
        let (runner, prompts) = RecordingRunner::new([pass_with_notes("output"), pass()]);
        PipelineExecutor::new(config, runner).run().unwrap();
        let second = &prompts.lock().unwrap()[1].clone();
        let notes_pos = second.find("<previous-stage>").unwrap();
        let base_pos = second.find("task-b").unwrap();
        assert!(notes_pos < base_pos, "note block must precede base prompt");
    }

    fn run_summary(config: PipelineConfig, runner: FakeRunner) -> RunSummary {
        PipelineExecutor::new(config, runner)
            .run()
            .unwrap()
            .0
            .summary
    }

    #[test]
    fn terminal_reason_done_for_completed_pipeline() {
        let config = pipeline(vec![single_stage_entry(stage("a"))]);
        let s = run_summary(config, FakeRunner::new([pass()]));
        assert_eq!(s.terminal_reason, TerminalReason::Done);
    }

    #[test]
    fn terminal_reason_exit_for_pass_route() {
        let mut s = stage("a");
        s.on_pass = OnPass::Exit;
        let config = pipeline(vec![single_stage_entry(s)]);
        let summary = run_summary(config, FakeRunner::new([pass()]));
        assert_eq!(summary.terminal_reason, TerminalReason::Exit);
    }

    #[test]
    fn terminal_reason_fail_exit_for_fail_route() {
        let config = pipeline(vec![single_stage_entry(stage("a"))]);
        let summary = run_summary(config, FakeRunner::new([fail()]));
        assert!(
            matches!(summary.terminal_reason, TerminalReason::FailExit { .. }),
            "expected FailExit, got {:?}",
            summary.terminal_reason
        );
    }

    #[test]
    fn terminal_reason_cap_hit_for_multi_stage_loop_cap() {
        let config = pipeline(vec![PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(1),
            stages: vec![stage("a")],
        })]);
        let summary = run_summary(config, FakeRunner::new([pass()]));
        assert_eq!(summary.terminal_reason, TerminalReason::CapHit);
    }

    #[test]
    fn last_stage_and_verdict_tracked() {
        let config = pipeline(vec![
            single_stage_entry(stage("first")),
            single_stage_entry(stage("last")),
        ]);
        let pass_with_notes = Some(Verdict {
            status: VerdictStatus::Pass,
            notes: Some("done".to_string()),
        });
        let runner = FakeRunner::new([pass(), pass_with_notes.clone()]);
        let summary = run_summary(config, runner);
        assert_eq!(summary.last_stage.as_deref(), Some("last"));
        assert_eq!(summary.last_verdict, pass_with_notes);
    }

    #[test]
    fn loop_iteration_count_captured_in_summary() {
        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(3),
            stages: vec![stage("a")],
        });
        let config = pipeline(vec![loop_entry]);
        // 3 passes → cap at iteration 4 → summary records 3 loop iterations
        let summary = run_summary(config, FakeRunner::new([pass(), pass(), pass()]));
        assert_eq!(summary.iteration_counters.loops.get(&0), Some(&3));
        assert_eq!(summary.iteration_counters.global, 3);
    }

    #[test]
    fn cap_hit_identifies_loop_when_loop_max_iteration_exceeded() {
        let config = pipeline(vec![PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(1),
            stages: vec![stage("a")],
        })]);
        let summary = run_summary(config, FakeRunner::new([pass()]));
        assert!(
            matches!(
                summary.cap_hit,
                Some(CapHitKind::LoopMaxIteration { loop_idx: 0, .. })
            ),
            "expected LoopMaxIteration(0), got {:?}",
            summary.cap_hit
        );
    }

    #[test]
    fn cap_hit_identifies_global_when_max_pipeline_exceeded() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_stages: 2,
        };
        let summary = run_summary(config, FakeRunner::new([fail(), fail(), fail()]));
        assert!(
            matches!(summary.cap_hit, Some(CapHitKind::MaxStages { .. })),
            "expected MaxStages, got {:?}",
            summary.cap_hit
        );
    }

    fn run_result(config: PipelineConfig, runner: FakeRunner) -> PipelineRunResult {
        PipelineExecutor::new(config, runner).run().unwrap().0
    }

    #[test]
    fn pipeline_state_captures_current_idx_on_fail_exit() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
        ]);
        // a passes, b fails → FailExit; current_idx should point to b (index 1)
        let result = run_result(config, FakeRunner::new([pass(), fail()]));
        assert_eq!(result.pipeline_state.current_idx, 1);
    }

    #[test]
    fn pipeline_state_captures_retry_counts_cleared_on_pass() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = 5;
        let config = pipeline(vec![single_stage_entry(s)]);
        // Two fails, then pass — retry_counts cleared on pass
        let result = run_result(config, FakeRunner::new([fail(), fail(), pass()]));
        assert_eq!(result.pipeline_state.retry_counts.get("a"), None);
    }

    #[test]
    fn pipeline_state_captures_retry_counts_on_exit() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = 1;
        let config = pipeline(vec![single_stage_entry(s)]);
        // Two fails exceed max_retries → exit; retry_count for "a" should be 2
        let result = run_result(config, FakeRunner::new([fail(), fail()]));
        assert_eq!(result.pipeline_state.retry_counts.get("a"), Some(&2));
    }

    #[test]
    fn retry_counts_not_incremented_for_on_fail_stage() {
        let mut s = stage("a");
        s.on_fail = OnFail::Stage("a".to_string()); // route back to self
        let config = pipeline(vec![single_stage_entry(s)]);
        // Two fails (routing back), then pass — retry_counts must stay empty
        let result = run_result(config, FakeRunner::new([fail(), fail(), pass()]));
        assert_eq!(
            result.pipeline_state.retry_counts.get("a"),
            None,
            "retry_counts must not increment for on_fail: stage(...)"
        );
    }

    #[test]
    fn retry_counts_not_incremented_for_on_fail_exit() {
        let config = pipeline(vec![single_stage_entry(stage("a"))]);
        // Default on_fail is Exit; one fail → exit
        let result = run_result(config, FakeRunner::new([fail()]));
        assert_eq!(
            result.pipeline_state.retry_counts.get("a"),
            None,
            "retry_counts must not increment for on_fail: exit"
        );
    }

    #[test]
    fn retry_counts_incremented_only_for_on_fail_retry() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = 10;
        let config = pipeline(vec![single_stage_entry(s)]);
        // Three fails, then pass — retry_counts should have been 3 before pass cleared it
        // After pass, it's cleared; check it was tracking by verifying Done outcome
        let result = run_result(config, FakeRunner::new([fail(), fail(), fail(), pass()]));
        assert_eq!(result.outcome, PipelineOutcome::Done);
        assert_eq!(
            result.pipeline_state.retry_counts.get("a"),
            None,
            "retry_counts cleared on pass"
        );
    }

    #[test]
    fn resume_restores_state_and_continues() {
        // Pipeline: a, b, c — first run completes a (pass), then b fails → FailExit
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
            single_stage_entry(stage("c")),
        ]);
        let first_run = run_result(config.clone(), FakeRunner::new([pass(), fail()]));
        assert_eq!(first_run.pipeline_state.current_idx, 1);

        // Resume from saved state: b should run first (pass), then c (pass) → Done
        let state = first_run.pipeline_state;
        let (result, _) =
            PipelineExecutor::resume(config, FakeRunner::new([pass(), pass()]), state)
                .run()
                .unwrap();
        assert_eq!(result.outcome, PipelineOutcome::Done);
    }

    #[test]
    fn resume_preserves_last_stage_and_verdict_for_note_injection() {
        let mut a = stage("a");
        a.prompt = Some("task-a".to_string());
        let mut b = stage("b");
        b.prompt = Some("task-b".to_string());
        let config = pipeline(vec![single_stage_entry(a), single_stage_entry(b)]);

        let (first_run, _) = PipelineExecutor::new(config.clone(), FakeRunner::new([fail()]))
            .run()
            .unwrap();
        let state = first_run.pipeline_state.clone();
        assert_eq!(state.last_stage, Some("a".to_string()));
        assert_eq!(state.last_verdict, fail());

        // Resume: a ran, failed; resume should start at a again with last_verdict from first run
        let state2 = PipelineState {
            current_idx: 0,
            last_stage: Some("a".to_string()),
            last_verdict: Some(crate::verdict::Verdict {
                status: crate::verdict::VerdictStatus::Fail,
                notes: Some("needs fix".to_string()),
            }),
            ..first_run.pipeline_state
        };
        let (runner, prompts) = RecordingRunner::new([pass(), pass()]);
        PipelineExecutor::resume(config, runner, state2)
            .run()
            .unwrap();
        let prompts = prompts.lock().unwrap();
        // First resumed stage should have note block from previous run
        assert!(
            prompts[0].contains("<previous-stage>"),
            "resumed first stage must have note block"
        );
        assert!(prompts[0].contains("needs fix"));
    }

    #[test]
    fn resume_global_counter_preserved() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = 10;
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_stages: 5,
        };
        // First run: 3 fails → CapHit at max 3
        let config3 = PipelineConfig {
            entries: config.entries.clone(),
            max_stages: 3,
        };
        let first_run = run_result(config3, FakeRunner::new([fail(), fail(), fail()]));
        assert_eq!(first_run.pipeline_state.global_counter, 3);

        // Resume with higher limit: counter starts at 3, 2 more iterations available
        let state = first_run.pipeline_state;
        let (result, _) =
            PipelineExecutor::resume(config, FakeRunner::new([fail(), pass()]), state)
                .run()
                .unwrap();
        assert_eq!(result.outcome, PipelineOutcome::Done);
    }

    #[test]
    fn top_level_on_fail_routes_to_loop_internal_stage() {
        let mut pr_reviewer = stage("pr-reviewer");
        pr_reviewer.on_fail = OnFail::Stage("implementer".to_string());

        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: None,
            stages: vec![stage("implementer"), stage("reviewer")],
        });

        let config = pipeline(vec![single_stage_entry(pr_reviewer), loop_entry]);
        // pr-reviewer fails → route to implementer (loop stage 0)
        // loop: implementer pass, reviewer pass → loop end → pipeline Done
        assert_eq!(
            run_outcome(config, FakeRunner::new([fail(), pass(), done()])),
            PipelineOutcome::Done
        );
    }

    #[test]
    fn top_level_on_pass_routes_to_loop_internal_non_first_stage() {
        let mut dispatcher = stage("dispatcher");
        dispatcher.on_pass = OnPass::Stage("reviewer".to_string());

        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: None,
            stages: vec![stage("implementer"), stage("reviewer")],
        });

        let config = pipeline(vec![single_stage_entry(dispatcher), loop_entry]);
        // dispatcher passes → route to reviewer (loop stage 1)
        // loop: reviewer passes → wraps to implementer → implementer done → pipeline Done
        assert_eq!(
            run_outcome(config, FakeRunner::new([pass(), pass(), done()])),
            PipelineOutcome::Done
        );
    }

    #[test]
    fn cross_scope_routing_respects_max_iteration() {
        let mut trigger = stage("trigger");
        trigger.on_pass = OnPass::Stage("implementer".to_string());

        let loop_entry = PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(1),
            stages: vec![stage("implementer"), stage("reviewer")],
        });

        let config = pipeline(vec![single_stage_entry(trigger), loop_entry]);
        // trigger passes → route to implementer (stage 0, iteration_count → 1)
        // implementer pass, reviewer pass → wraps → iteration 2 > 1 → CapHit
        assert_eq!(
            run_outcome(config, FakeRunner::new([pass(), pass(), pass()])),
            PipelineOutcome::CapHit
        );
    }

    // ── PipelineState serialization ────────────────────────────────────────────

    fn full_state() -> PipelineState {
        let mut retry_counts = HashMap::new();
        retry_counts.insert("stage-a".to_string(), 2u32);
        let mut failure_totals = HashMap::new();
        failure_totals.insert("stage-a".to_string(), 3u32);
        let mut loop_iters = HashMap::new();
        loop_iters.insert(0usize, 3u32);
        PipelineState {
            current_idx: 2,
            global_counter: 7,
            retry_counts,
            failure_totals,
            last_stage: Some("stage-a".to_string()),
            last_verdict: Some(Verdict {
                status: VerdictStatus::Fail,
                notes: Some("oops".to_string()),
            }),
            loop_iterations: loop_iters,
            env: vec![("KEY".to_string(), "val".to_string())],
        }
    }

    #[test]
    fn pipeline_state_to_json_produces_expected_shape() {
        let state = full_state();
        let v = serde_json::to_value(&state).unwrap();
        assert_eq!(v["current_idx"], 2);
        assert_eq!(v["global_counter"], 7);
        assert_eq!(v["retry_counts"]["stage-a"], 2);
        assert_eq!(v["failure_totals"]["stage-a"], 3);
        assert_eq!(v["last_stage"], "stage-a");
        assert_eq!(v["last_verdict"]["status"], "fail");
        assert_eq!(v["last_verdict"]["notes"], "oops");
        assert_eq!(v["loop_iterations"]["0"], 3);
        assert_eq!(v["env"]["KEY"], "val");
    }

    #[test]
    fn pipeline_state_round_trips_via_json() {
        let original = full_state();
        let json = serde_json::to_value(&original).unwrap();
        let restored: PipelineState = serde_json::from_value(json).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn from_json_errors_on_missing_current_idx() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v.as_object_mut().unwrap().remove("current_idx");
        assert!(serde_json::from_value::<PipelineState>(v).is_err());
    }

    #[test]
    fn from_json_errors_on_missing_global_counter() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v.as_object_mut().unwrap().remove("global_counter");
        assert!(serde_json::from_value::<PipelineState>(v).is_err());
    }

    #[test]
    fn from_json_errors_on_missing_retry_counts() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v.as_object_mut().unwrap().remove("retry_counts");
        assert!(serde_json::from_value::<PipelineState>(v).is_err());
    }

    #[test]
    fn from_json_errors_on_non_number_in_retry_counts() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v["retry_counts"]["stage-a"] = serde_json::json!("not-a-number");
        assert!(serde_json::from_value::<PipelineState>(v).is_err());
    }

    #[test]
    fn from_json_errors_on_missing_loop_iterations() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v.as_object_mut().unwrap().remove("loop_iterations");
        assert!(serde_json::from_value::<PipelineState>(v).is_err());
    }

    #[test]
    fn from_json_errors_on_non_number_in_loop_iterations() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v["loop_iterations"]["0"] = serde_json::json!("not-a-number");
        assert!(serde_json::from_value::<PipelineState>(v).is_err());
    }

    #[test]
    fn from_json_errors_on_malformed_last_verdict() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v["last_verdict"] = serde_json::json!({"status": 42});
        assert!(serde_json::from_value::<PipelineState>(v).is_err());
    }

    #[test]
    fn from_json_env_defaults_to_empty_when_absent() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v.as_object_mut().unwrap().remove("env");
        let restored: PipelineState = serde_json::from_value(v).unwrap();
        assert_eq!(restored.env, vec![]);
    }

    #[test]
    fn from_json_errors_on_non_string_in_env() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v["env"]["KEY"] = serde_json::json!(42);
        assert!(serde_json::from_value::<PipelineState>(v).is_err());
    }

    #[test]
    fn from_json_failure_totals_defaults_to_empty_when_absent() {
        let mut v = serde_json::to_value(full_state()).unwrap();
        v.as_object_mut().unwrap().remove("failure_totals");
        let restored: PipelineState = serde_json::from_value(v).unwrap();
        assert_eq!(restored.failure_totals, HashMap::new());
    }

    #[test]
    fn pipeline_state_deserializes_legacy_fail_counts_key() {
        let json = serde_json::json!({
            "current_idx": 1,
            "global_counter": 3,
            "fail_counts": {"stage-a": 2},
            "failure_totals": {},
            "last_stage": null,
            "last_verdict": null,
            "loop_iterations": {},
            "env": {}
        });
        let state: PipelineState = serde_json::from_value(json).unwrap();
        assert_eq!(state.retry_counts.get("stage-a"), Some(&2));
    }

    // ── max_failure tests ──────────────────────────────────────────────────────

    fn run_result_with_state(
        config: PipelineConfig,
        runner: FakeRunner,
    ) -> (PipelineRunResult, FakeRunner) {
        PipelineExecutor::new(config, runner).run().unwrap()
    }

    #[test]
    fn max_failure_none_allows_unbounded_failures() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = 100;
        s.max_failure = None;
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_stages: 5,
        };
        // 5 fails hit the max_stages cap, not max_failure
        assert_eq!(
            run_outcome(
                config,
                FakeRunner::new([fail(), fail(), fail(), fail(), fail()])
            ),
            PipelineOutcome::CapHit
        );
    }

    #[test]
    fn max_failure_exits_after_limit_exceeded() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = 100;
        s.max_failure = Some(2);
        let config = pipeline(vec![single_stage_entry(s)]);
        // 3 total failures: failure_totals reaches 3 > 2, exit on 3rd fail
        assert_eq!(
            run_outcome(config, FakeRunner::new([fail(), fail(), fail()])),
            PipelineOutcome::Exit { from_fail: true }
        );
    }

    #[test]
    fn max_failure_increments_on_every_on_fail_strategy() {
        // on_fail: exit — each fail increments failure_totals
        let mut s = stage("a");
        s.on_fail = OnFail::Exit;
        s.max_failure = Some(1);
        let config = pipeline(vec![single_stage_entry(s)]);
        // first fail: failure_totals=1, not exceeded (1 > 1 is false) → on_fail:exit triggers
        assert_eq!(
            run_outcome(config.clone(), FakeRunner::new([fail()])),
            PipelineOutcome::Exit { from_fail: true }
        );

        // verify failure_totals tracks across on_fail:stage routing
        let mut a = stage("a");
        a.on_fail = OnFail::Stage("b".to_string());
        a.max_failure = Some(1);
        let b = stage("b");
        let config2 = pipeline(vec![single_stage_entry(a), single_stage_entry(b)]);
        // a fails (totals=1, ok) → route to b; b passes → done
        assert_eq!(
            run_outcome(config2, FakeRunner::new([fail(), pass()])),
            PipelineOutcome::Done
        );
    }

    #[test]
    fn max_failure_does_not_reset_on_pass() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.on_pass = OnPass::Stage("a".to_string()); // loop back so we can fail again after a pass
        s.max_retries = 100;
        s.max_failure = Some(3);
        let config = pipeline(vec![single_stage_entry(s)]);
        // fail, fail, pass (reset retry_counts but NOT failure_totals[=2]), fail, fail → totals=4>3 exit
        assert_eq!(
            run_outcome(
                config,
                FakeRunner::new([fail(), fail(), pass(), fail(), fail()])
            ),
            PipelineOutcome::Exit { from_fail: true }
        );
    }

    #[test]
    fn failure_totals_persisted_in_pipeline_state() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = 100;
        s.max_failure = Some(5);
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_stages: 3,
        };
        let (result, _) = run_result_with_state(config, FakeRunner::new([fail(), fail(), fail()]));
        assert_eq!(
            result.pipeline_state.failure_totals.get("a").copied(),
            Some(3)
        );
    }
}
