use std::collections::HashMap;

use crate::config::{LoopConfig, OnFail, OnPass, PipelineConfig, PipelineEntry, StageConfig};
use crate::verdict::{Verdict, VerdictStatus};

pub trait StageRunner {
    fn run(&mut self, stage_name: &str, prompt: &str, model: Option<&str>) -> Option<Verdict>;
}

/// High-level outcome of a pipeline run.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineOutcome {
    /// All top-level stages completed, or a `done` verdict was emitted outside a loop.
    Done,
    /// An `exit` route was triggered; `from_fail` distinguishes fail-route vs pass-route.
    Exit { from_fail: bool },
    /// An iteration cap was exceeded.
    CapHit,
}

/// Human-readable terminal reason written to the summary artifact.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalReason {
    /// Pipeline completed normally (done verdict or all stages passed).
    Done,
    /// Exit via a pass-route (`on_pass: exit`).
    Exit,
    /// Exit via a fail-route (`on_fail: exit` or max_retries exceeded).
    FailExit,
    /// Iteration cap hit in a multi-stage pipeline.
    CapHit,
    /// Flat-form iteration limit reached (expected terminal for flat configs).
    Ok,
}

/// Identifies which counter tripped when `CapHit` is the terminal reason.
#[derive(Debug, Clone, PartialEq)]
pub enum CapHitKind {
    /// A loop's `max_iteration` was exceeded; `loop_idx` is the entry index.
    LoopMaxIteration(usize),
    /// The global `max_pipeline_iterations` was exceeded.
    MaxPipelineIterations,
}

/// Snapshot of iteration counters at pipeline exit.
#[derive(Debug, Clone, PartialEq)]
pub struct IterationCounters {
    pub global: u32,
    /// Per-loop iteration counts, keyed by the loop's index in the top-level entries list.
    pub loops: HashMap<usize, u32>,
}

/// Runtime state snapshot used to resume an interrupted pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineState {
    pub current_idx: usize,
    pub global_counter: u32,
    pub fail_counts: HashMap<String, u32>,
    pub last_stage: Option<String>,
    pub last_verdict: Option<crate::verdict::Verdict>,
    pub loop_iterations: HashMap<usize, u32>,
}

/// Mutable progress tracked across stage invocations during a pipeline run.
struct PipelineProgress {
    fail_counts: HashMap<String, u32>,
    global_counter: u32,
    input: Option<String>,
    last_stage: Option<String>,
    last_verdict: Option<Verdict>,
}

/// Execution summary produced by every `PipelineExecutor::run` call.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    pub terminal_reason: TerminalReason,
    pub last_stage: Option<String>,
    pub last_verdict: Option<Verdict>,
    pub iteration_counters: IterationCounters,
    /// Which counter tripped, if `terminal_reason` is `CapHit` or `Ok`.
    pub cap_hit: Option<CapHitKind>,
    pub session_id: Option<String>,
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
}

impl<R: StageRunner> PipelineExecutor<R> {
    pub fn new(config: PipelineConfig, runner: R) -> Self {
        Self {
            config,
            runner,
            input: None,
            initial_state: None,
        }
    }

    pub fn resume(config: PipelineConfig, runner: R, state: PipelineState) -> Self {
        Self {
            config,
            runner,
            input: None,
            initial_state: Some(state),
        }
    }

    pub fn with_input(mut self, input: Option<String>) -> Self {
        self.input = input;
        self
    }

    pub fn run(mut self) -> PipelineRunResult {
        let name_to_entry = build_name_index(&self.config);
        let max_pipeline = self.config.max_pipeline_iterations;
        let is_flat_form = self.config.is_flat_form;

        let (mut current_idx, mut loop_iterations, mut progress) = match self.initial_state.take() {
            Some(s) => (
                s.current_idx,
                s.loop_iterations,
                PipelineProgress {
                    fail_counts: s.fail_counts,
                    global_counter: s.global_counter,
                    input: self.input.take(),
                    last_stage: s.last_stage,
                    last_verdict: s.last_verdict,
                },
            ),
            None => (
                0,
                HashMap::new(),
                PipelineProgress {
                    fail_counts: HashMap::new(),
                    global_counter: 0,
                    input: self.input.take(),
                    last_stage: None,
                    last_verdict: None,
                },
            ),
        };

        let (outcome, cap_hit) = loop {
            if current_idx >= self.config.entries.len() {
                break (PipelineOutcome::Done, None);
            }

            match &self.config.entries[current_idx] {
                PipelineEntry::Stage(stage) => {
                    if progress.global_counter >= max_pipeline {
                        break (
                            PipelineOutcome::CapHit,
                            Some(CapHitKind::MaxPipelineIterations),
                        );
                    }
                    progress.global_counter += 1;
                    match run_stage(&mut self.runner, stage, &name_to_entry, &mut progress) {
                        StageOutcome::Advance(next_idx) => current_idx = next_idx,
                        StageOutcome::AdvanceIntoLoop {
                            entry_idx,
                            stage_idx,
                        } => {
                            let PipelineEntry::Loop(loop_config) = &self.config.entries[entry_idx]
                            else {
                                unreachable!("AdvanceIntoLoop references non-loop entry");
                            };
                            let outcome = run_loop(
                                &mut self.runner,
                                loop_config,
                                &mut progress,
                                max_pipeline,
                                stage_idx,
                            );
                            match handle_loop_outcome(outcome, entry_idx, &mut loop_iterations) {
                                LoopControl::Advance(next) => current_idx = next,
                                LoopControl::Break(o, cap) => break (o, cap),
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
                    let outcome = run_loop(
                        &mut self.runner,
                        loop_config,
                        &mut progress,
                        max_pipeline,
                        0,
                    );
                    match handle_loop_outcome(outcome, entry_idx, &mut loop_iterations) {
                        LoopControl::Advance(next) => current_idx = next,
                        LoopControl::Break(o, cap) => break (o, cap),
                    }
                }
            }
        };

        let terminal_reason = match (&outcome, is_flat_form) {
            (PipelineOutcome::Done, _) => TerminalReason::Done,
            (PipelineOutcome::Exit { from_fail: false }, _) => TerminalReason::Exit,
            (PipelineOutcome::Exit { from_fail: true }, _) => TerminalReason::FailExit,
            (PipelineOutcome::CapHit, true) => TerminalReason::Ok,
            (PipelineOutcome::CapHit, false) => TerminalReason::CapHit,
        };

        let pipeline_state = PipelineState {
            current_idx,
            global_counter: progress.global_counter,
            fail_counts: progress.fail_counts,
            last_stage: progress.last_stage.clone(),
            last_verdict: progress.last_verdict.clone(),
            loop_iterations: loop_iterations.clone(),
        };

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
        }
    }
}

#[derive(Clone)]
enum ExitKind {
    PassRoute,
    FailRoute,
}

enum LoopCapKind {
    MaxIteration,
    MaxPipelineIterations,
}

enum StageOutcome {
    Advance(usize),
    AdvanceIntoLoop { entry_idx: usize, stage_idx: usize },
    Done,
    Exit(ExitKind),
}

/// Resolved destination of a named route target.
enum RouteTarget {
    Entry(usize),
    LoopStage { entry_idx: usize, stage_idx: usize },
}

/// Control-flow result of a completed loop run.
enum LoopControl {
    Advance(usize),
    Break(PipelineOutcome, Option<CapHitKind>),
}

enum LoopOutcome {
    LoopDone { iterations: u32 },
    Exit { kind: ExitKind, iterations: u32 },
    CapHit { kind: LoopCapKind, iterations: u32 },
}

fn run_loop(
    runner: &mut dyn StageRunner,
    loop_config: &LoopConfig,
    progress: &mut PipelineProgress,
    max_pipeline: u32,
    start_stage_idx: usize,
) -> LoopOutcome {
    let loop_name_to_idx: HashMap<String, usize> = loop_config
        .stages
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.clone(), i))
        .collect();

    let mut iteration_count: u32 = 0;
    let mut stage_idx: usize = start_stage_idx;
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
                    return LoopOutcome::CapHit {
                        kind: LoopCapKind::MaxIteration,
                        iterations: iteration_count - 1,
                    };
                }
            }
        }
        retrying_top = false;

        if progress.global_counter >= max_pipeline {
            return LoopOutcome::CapHit {
                kind: LoopCapKind::MaxPipelineIterations,
                iterations: iteration_count,
            };
        }
        progress.global_counter += 1;

        let stage = &loop_config.stages[stage_idx];
        let base_prompt = stage.prompt.as_deref().unwrap_or("");
        let with_input = inject_input(&mut progress.input, base_prompt);
        let effective_prompt = inject_note_block(
            progress.last_stage.as_deref(),
            &progress.last_verdict,
            &with_input,
        );
        progress.last_stage = Some(stage.name.clone());
        let verdict = runner.run(&stage.name, &effective_prompt, stage.model.as_deref());
        progress.last_verdict = verdict.clone();

        if matches!(
            verdict.as_ref().map(|v| &v.status),
            Some(VerdictStatus::Done)
        ) {
            return LoopOutcome::LoopDone {
                iterations: iteration_count,
            };
        }

        let is_pass = matches!(
            verdict.as_ref().map(|v| &v.status),
            Some(VerdictStatus::Pass)
        );

        if is_pass {
            progress.fail_counts.remove(&stage.name);
            match &stage.on_pass {
                OnPass::Next => stage_idx += 1,
                OnPass::Stage(name) => match loop_name_to_idx.get(name.as_str()) {
                    Some(&idx) => stage_idx = idx,
                    None => {
                        return LoopOutcome::Exit {
                            kind: ExitKind::PassRoute,
                            iterations: iteration_count,
                        }
                    }
                },
                OnPass::Exit => {
                    return LoopOutcome::Exit {
                        kind: ExitKind::PassRoute,
                        iterations: iteration_count,
                    }
                }
            }
        } else {
            let fail_count = progress.fail_counts.entry(stage.name.clone()).or_insert(0);
            *fail_count += 1;

            if let Some(max) = stage.max_retries {
                if *fail_count > max {
                    return LoopOutcome::Exit {
                        kind: ExitKind::FailRoute,
                        iterations: iteration_count,
                    };
                }
            }

            match &stage.on_fail {
                OnFail::Exit => {
                    return LoopOutcome::Exit {
                        kind: ExitKind::FailRoute,
                        iterations: iteration_count,
                    }
                }
                OnFail::Retry => {
                    if stage_idx == 0 {
                        retrying_top = true;
                    }
                }
                OnFail::Stage(name) => match loop_name_to_idx.get(name.as_str()) {
                    Some(&idx) => stage_idx = idx,
                    None => {
                        return LoopOutcome::Exit {
                            kind: ExitKind::FailRoute,
                            iterations: iteration_count,
                        }
                    }
                },
            }
        }
    }
}

/// Builds a map from stage name to its resolved route target.
/// Top-level stages map to `Entry(i)`; loop-internal stages map to `LoopStage`.
fn build_name_index(config: &PipelineConfig) -> HashMap<String, RouteTarget> {
    let mut map = HashMap::new();
    for (i, entry) in config.entries.iter().enumerate() {
        match entry {
            PipelineEntry::Stage(s) => {
                map.insert(s.name.clone(), RouteTarget::Entry(i));
            }
            PipelineEntry::Loop(l) => {
                for (j, s) in l.stages.iter().enumerate() {
                    map.insert(
                        s.name.clone(),
                        RouteTarget::LoopStage {
                            entry_idx: i,
                            stage_idx: j,
                        },
                    );
                }
            }
        }
    }
    map
}

fn handle_loop_outcome(
    outcome: LoopOutcome,
    entry_idx: usize,
    loop_iterations: &mut HashMap<usize, u32>,
) -> LoopControl {
    match outcome {
        LoopOutcome::LoopDone { iterations } => {
            loop_iterations.insert(entry_idx, iterations);
            LoopControl::Advance(entry_idx + 1)
        }
        LoopOutcome::Exit { kind, iterations } => {
            loop_iterations.insert(entry_idx, iterations);
            LoopControl::Break(
                PipelineOutcome::Exit {
                    from_fail: matches!(kind, ExitKind::FailRoute),
                },
                None,
            )
        }
        LoopOutcome::CapHit {
            kind: LoopCapKind::MaxIteration,
            iterations,
        } => {
            loop_iterations.insert(entry_idx, iterations);
            LoopControl::Break(
                PipelineOutcome::CapHit,
                Some(CapHitKind::LoopMaxIteration(entry_idx)),
            )
        }
        LoopOutcome::CapHit {
            kind: LoopCapKind::MaxPipelineIterations,
            iterations,
        } => {
            loop_iterations.insert(entry_idx, iterations);
            LoopControl::Break(
                PipelineOutcome::CapHit,
                Some(CapHitKind::MaxPipelineIterations),
            )
        }
    }
}

/// Prepends `<capsule:input>` block if `input` is Some, consuming it.
fn inject_input(input: &mut Option<String>, base_prompt: &str) -> String {
    if let Some(text) = input.take() {
        format!("<capsule:input>\n{text}\n</capsule:input>\n\n{base_prompt}")
    } else {
        base_prompt.to_string()
    }
}

/// Prepends `<previous-stage>` block from the last verdict when notes are present.
fn inject_note_block(
    last_stage: Option<&str>,
    last_verdict: &Option<Verdict>,
    base_prompt: &str,
) -> String {
    if let (Some(name), Some(verdict)) = (last_stage, last_verdict.as_ref()) {
        if let Some(block) =
            crate::note_block::format(name, &verdict.status, verdict.notes.as_deref())
        {
            return format!("{block}\n\n{base_prompt}");
        }
    }
    base_prompt.to_string()
}

fn run_stage(
    runner: &mut dyn StageRunner,
    stage: &StageConfig,
    name_to_entry: &HashMap<String, RouteTarget>,
    progress: &mut PipelineProgress,
) -> StageOutcome {
    let base_prompt = stage.prompt.as_deref().unwrap_or("");
    let with_input = inject_input(&mut progress.input, base_prompt);
    let effective_prompt = inject_note_block(
        progress.last_stage.as_deref(),
        &progress.last_verdict,
        &with_input,
    );
    progress.last_stage = Some(stage.name.clone());
    let verdict = runner.run(&stage.name, &effective_prompt, stage.model.as_deref());
    progress.last_verdict = verdict.clone();

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
        progress.fail_counts.remove(&stage.name);
        route_pass(stage, name_to_entry)
    } else {
        let fail_count = progress.fail_counts.entry(stage.name.clone()).or_insert(0);
        *fail_count += 1;

        if let Some(max) = stage.max_retries {
            if *fail_count > max {
                return StageOutcome::Exit(ExitKind::FailRoute);
            }
        }

        route_fail(stage, name_to_entry)
    }
}

fn route_pass(stage: &StageConfig, name_to_entry: &HashMap<String, RouteTarget>) -> StageOutcome {
    match &stage.on_pass {
        OnPass::Next => {
            let idx = match name_to_entry.get(&stage.name) {
                Some(RouteTarget::Entry(i)) => *i,
                _ => 0,
            };
            StageOutcome::Advance(idx + 1)
        }
        OnPass::Stage(name) => match name_to_entry.get(name.as_str()) {
            Some(RouteTarget::Entry(idx)) => StageOutcome::Advance(*idx),
            Some(RouteTarget::LoopStage {
                entry_idx,
                stage_idx,
            }) => StageOutcome::AdvanceIntoLoop {
                entry_idx: *entry_idx,
                stage_idx: *stage_idx,
            },
            None => StageOutcome::Exit(ExitKind::PassRoute),
        },
        OnPass::Exit => StageOutcome::Exit(ExitKind::PassRoute),
    }
}

fn route_fail(stage: &StageConfig, name_to_entry: &HashMap<String, RouteTarget>) -> StageOutcome {
    match &stage.on_fail {
        OnFail::Exit => StageOutcome::Exit(ExitKind::FailRoute),
        OnFail::Retry => {
            let idx = match name_to_entry.get(&stage.name) {
                Some(RouteTarget::Entry(i)) => *i,
                _ => 0,
            };
            StageOutcome::Advance(idx)
        }
        OnFail::Stage(name) => match name_to_entry.get(name.as_str()) {
            Some(RouteTarget::Entry(idx)) => StageOutcome::Advance(*idx),
            Some(RouteTarget::LoopStage {
                entry_idx,
                stage_idx,
            }) => StageOutcome::AdvanceIntoLoop {
                entry_idx: *entry_idx,
                stage_idx: *stage_idx,
            },
            None => StageOutcome::Exit(ExitKind::FailRoute),
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
            is_flat_form: false,
        }
    }

    fn single_stage_entry(s: StageConfig) -> PipelineEntry {
        PipelineEntry::Stage(s)
    }

    fn run_outcome(config: PipelineConfig, runner: FakeRunner) -> PipelineOutcome {
        PipelineExecutor::new(config, runner).run().outcome
    }

    // Linear three-stage happy path: all pass, pipeline reaches Done.
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

    // on_fail: exit (default) terminates pipeline on first fail.
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

    // on_fail: retry — stage retries until pass.
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

    // on_fail: retry — max_retries exceeded causes exit.
    #[test]
    fn on_fail_retry_exits_when_max_retries_exceeded() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = Some(2);
        let config = pipeline(vec![single_stage_entry(s)]);
        // 3 fails: fail_count reaches 3 > 2, exit
        assert_eq!(
            run_outcome(config, FakeRunner::new([fail(), fail(), fail()])),
            PipelineOutcome::Exit { from_fail: true }
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
        assert_eq!(
            run_outcome(
                config,
                FakeRunner::new([pass(), fail(), pass(), pass(), pass()])
            ),
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
        assert_eq!(
            run_outcome(
                config,
                FakeRunner::new([fail(), fail(), pass(), fail(), fail(), pass()])
            ),
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
            is_flat_form: false,
        };
        // 4 fails: 3rd triggers cap, 4th never runs
        assert_eq!(
            run_outcome(config, FakeRunner::new([fail(), fail(), fail(), fail()])),
            PipelineOutcome::CapHit
        );
    }

    // Silent exit (no verdict) is treated as implicit fail and routes via on_fail.
    #[test]
    fn silent_exit_treated_as_implicit_fail() {
        let config = pipeline(vec![single_stage_entry(stage("a"))]);
        // None → implicit fail → on_fail: exit → Exit { from_fail: true }
        assert_eq!(
            run_outcome(config, FakeRunner::new([None])),
            PipelineOutcome::Exit { from_fail: true }
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
        assert_eq!(
            run_outcome(config, FakeRunner::new([done(), pass()])),
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
        assert_eq!(
            run_outcome(config, FakeRunner::new([pass(), pass()])),
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
        assert_eq!(
            run_outcome(config, FakeRunner::new([pass(), fail(), pass(), pass()])),
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
            PipelineExecutor::new(config, runner).run().outcome,
            PipelineOutcome::Done
        );
    }

    // ── Recording runner for input-injection and summary tests ─────────────────

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
        ) -> Option<Verdict> {
            self.prompts.lock().unwrap().push(prompt.to_string());
            self.responses
                .pop_front()
                .expect("RecordingRunner: no more responses queued")
        }
    }

    // ── Input injection tests ───────────────────────────────────────────────────

    // Input is prepended to the first invocation only; absent on all subsequent calls.
    #[test]
    fn input_injected_on_first_invocation_only() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
        ]);
        let (runner, prompts) = RecordingRunner::new([pass(), pass()]);
        PipelineExecutor::new(config, runner)
            .with_input(Some("my-input".to_string()))
            .run();
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

    // No input: prompts are passed through unchanged.
    #[test]
    fn no_input_prompts_unchanged() {
        let mut s = stage("a");
        s.prompt = Some("hello".to_string());
        let config = pipeline(vec![single_stage_entry(s)]);
        let (runner, prompts) = RecordingRunner::new([pass()]);
        PipelineExecutor::new(config, runner).run();
        assert_eq!(prompts.lock().unwrap()[0], "hello");
    }

    // ── Note injection tests ────────────────────────────────────────────────────

    fn pass_with_notes(notes: &str) -> Option<Verdict> {
        Some(Verdict {
            status: VerdictStatus::Pass,
            notes: Some(notes.to_string()),
        })
    }

    // First stage never receives a note block.
    #[test]
    fn first_stage_has_no_note_block() {
        let config = pipeline(vec![single_stage_entry(stage("a"))]);
        let (runner, prompts) = RecordingRunner::new([pass_with_notes("first done")]);
        PipelineExecutor::new(config, runner).run();
        assert!(!prompts.lock().unwrap()[0].contains("<previous-stage>"));
    }

    // Second stage receives the note block from the first stage.
    #[test]
    fn second_stage_receives_note_block_from_first() {
        let mut a = stage("a");
        a.prompt = Some("prompt-a".to_string());
        let mut b = stage("b");
        b.prompt = Some("prompt-b".to_string());
        let config = pipeline(vec![single_stage_entry(a), single_stage_entry(b)]);
        let (runner, prompts) = RecordingRunner::new([pass_with_notes("result from a"), pass()]);
        PipelineExecutor::new(config, runner).run();
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

    // No note block when previous verdict has no notes.
    #[test]
    fn no_note_block_when_previous_verdict_has_no_notes() {
        let config = pipeline(vec![
            single_stage_entry(stage("a")),
            single_stage_entry(stage("b")),
        ]);
        let (runner, prompts) = RecordingRunner::new([pass(), pass()]);
        PipelineExecutor::new(config, runner).run();
        assert!(!prompts.lock().unwrap()[1].contains("<previous-stage>"));
    }

    // Inside a loop, the first stage of iteration 2 receives notes from the last stage of iteration 1.
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
        PipelineExecutor::new(config, runner).run();
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

    // Note block is placed before base prompt (and before any input block).
    #[test]
    fn note_block_ordering_notes_before_input_before_base() {
        let mut a = stage("a");
        a.prompt = Some("task-a".to_string());
        let mut b = stage("b");
        b.prompt = Some("task-b".to_string());
        let config = pipeline(vec![single_stage_entry(a), single_stage_entry(b)]);
        let (runner, prompts) = RecordingRunner::new([pass_with_notes("output"), pass()]);
        // No external input — just verify note block precedes base prompt
        PipelineExecutor::new(config, runner).run();
        let second = &prompts.lock().unwrap()[1].clone();
        let notes_pos = second.find("<previous-stage>").unwrap();
        let base_pos = second.find("task-b").unwrap();
        assert!(notes_pos < base_pos, "note block must precede base prompt");
    }

    // ── Terminal reason tests ───────────────────────────────────────────────────

    fn run_summary(config: PipelineConfig, runner: FakeRunner) -> RunSummary {
        PipelineExecutor::new(config, runner).run().summary
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
        // default on_fail: Exit — fail verdict triggers fail-route exit
        let summary = run_summary(config, FakeRunner::new([fail()]));
        assert_eq!(summary.terminal_reason, TerminalReason::FailExit);
    }

    #[test]
    fn terminal_reason_ok_for_flat_form_cap_hit() {
        let config = PipelineConfig {
            entries: vec![PipelineEntry::Loop(LoopConfig {
                max_iteration: Some(1),
                stages: vec![stage("a")],
            })],
            max_pipeline_iterations: 1000,
            is_flat_form: true,
        };
        // After 1 iteration the loop cap hits → TerminalReason::Ok for flat-form
        let summary = run_summary(config, FakeRunner::new([pass()]));
        assert_eq!(summary.terminal_reason, TerminalReason::Ok);
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

    // ── last_stage / last_verdict tracking ─────────────────────────────────────

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

    // ── Iteration counter tracking ─────────────────────────────────────────────

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

    // ── cap_hit tracking ──────────────────────────────────────────────────────

    #[test]
    fn cap_hit_identifies_loop_when_loop_max_iteration_exceeded() {
        let config = pipeline(vec![PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(1),
            stages: vec![stage("a")],
        })]);
        let summary = run_summary(config, FakeRunner::new([pass()]));
        assert_eq!(summary.cap_hit, Some(CapHitKind::LoopMaxIteration(0)));
    }

    #[test]
    fn cap_hit_identifies_global_when_max_pipeline_exceeded() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_pipeline_iterations: 2,
            is_flat_form: false,
        };
        let summary = run_summary(config, FakeRunner::new([fail(), fail(), fail()]));
        assert_eq!(summary.cap_hit, Some(CapHitKind::MaxPipelineIterations));
    }

    // ── PipelineState capture and resume ──────────────────────────────────────

    fn run_result(config: PipelineConfig, runner: FakeRunner) -> PipelineRunResult {
        PipelineExecutor::new(config, runner).run()
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
    fn pipeline_state_captures_fail_counts() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = Some(5);
        let config = pipeline(vec![single_stage_entry(s)]);
        // Two fails, then pass
        let result = run_result(config, FakeRunner::new([fail(), fail(), pass()]));
        assert_eq!(result.pipeline_state.fail_counts.get("a"), None);
    }

    #[test]
    fn pipeline_state_captures_fail_counts_on_exit() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        s.max_retries = Some(1);
        let config = pipeline(vec![single_stage_entry(s)]);
        // Two fails exceed max_retries → exit; fail_count for "a" should be 2
        let result = run_result(config, FakeRunner::new([fail(), fail()]));
        assert_eq!(result.pipeline_state.fail_counts.get("a"), Some(&2));
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
        let result =
            PipelineExecutor::resume(config, FakeRunner::new([pass(), pass()]), state).run();
        assert_eq!(result.outcome, PipelineOutcome::Done);
    }

    #[test]
    fn resume_preserves_last_stage_and_verdict_for_note_injection() {
        let mut a = stage("a");
        a.prompt = Some("task-a".to_string());
        let mut b = stage("b");
        b.prompt = Some("task-b".to_string());
        let config = pipeline(vec![single_stage_entry(a), single_stage_entry(b)]);

        let first_run = PipelineExecutor::new(config.clone(), FakeRunner::new([fail()])).run();
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
        PipelineExecutor::resume(config, runner, state2).run();
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
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_pipeline_iterations: 5,
            is_flat_form: false,
        };
        // First run: 3 fails → CapHit at max 3
        let config3 = PipelineConfig {
            entries: config.entries.clone(),
            max_pipeline_iterations: 3,
            is_flat_form: false,
        };
        let first_run = run_result(config3, FakeRunner::new([fail(), fail(), fail()]));
        assert_eq!(first_run.pipeline_state.global_counter, 3);

        // Resume with higher limit: counter starts at 3, 2 more iterations available
        let state = first_run.pipeline_state;
        let result =
            PipelineExecutor::resume(config, FakeRunner::new([fail(), pass()]), state).run();
        assert_eq!(result.outcome, PipelineOutcome::Done);
    }

    // ── Cross-scope routing tests ───────────────────────────────────────────────

    // Top-level on_fail routes into a loop-internal stage (the reported bug).
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

    // Top-level on_pass routes into a non-first loop-internal stage.
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

    // Cross-scope routing respects max_iteration — fresh iteration count on entry.
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
}
