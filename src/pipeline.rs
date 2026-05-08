use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::{LoopConfig, OnFail, OnPass, PipelineConfig, PipelineEntry, StageConfig};
use crate::verdict::{Verdict, VerdictStatus};
use anyhow::{anyhow, Context};

pub const SYSTEM_PREAMBLE: &str = include_str!("../base-image/system_preamble.md");

pub fn prepend_preamble(user_prompt: &str) -> String {
    format!("{SYSTEM_PREAMBLE}\n\n{user_prompt}")
}

fn read_prompt_file(capsule_dir: &Path, path_str: &str) -> anyhow::Result<String> {
    let path = capsule_dir.join(path_str);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("prompt file not found: {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

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
    /// Run environment pairs persisted for resume; omitted from disk on successful exits.
    pub env: Vec<(String, String)>,
}

impl PipelineState {
    pub fn to_json(&self) -> serde_json::Value {
        let fail_counts: serde_json::Map<String, serde_json::Value> = self
            .fail_counts
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();
        let loop_iterations: serde_json::Map<String, serde_json::Value> = self
            .loop_iterations
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
            .collect();
        let last_verdict = self
            .last_verdict
            .as_ref()
            .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null));
        let env: serde_json::Map<String, serde_json::Value> = self
            .env
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();
        serde_json::json!({
            "current_idx": self.current_idx,
            "global_counter": self.global_counter,
            "fail_counts": fail_counts,
            "last_stage": self.last_stage,
            "last_verdict": last_verdict,
            "loop_iterations": loop_iterations,
            "env": env,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> anyhow::Result<Self> {
        let current_idx = v["current_idx"]
            .as_u64()
            .ok_or_else(|| anyhow!("pipeline_state.current_idx missing or invalid"))?
            as usize;
        let global_counter = v["global_counter"]
            .as_u64()
            .ok_or_else(|| anyhow!("pipeline_state.global_counter missing or invalid"))?
            as u32;
        let fail_counts: HashMap<String, u32> = v["fail_counts"]
            .as_object()
            .ok_or_else(|| anyhow!("pipeline_state.fail_counts missing or not an object"))?
            .iter()
            .map(|(k, val)| {
                val.as_u64()
                    .ok_or_else(|| anyhow!("pipeline_state.fail_counts[{}] is not a number", k))
                    .map(|n| (k.clone(), n as u32))
            })
            .collect::<anyhow::Result<_>>()?;
        let last_stage = v["last_stage"].as_str().map(str::to_owned);
        let last_verdict: Option<crate::verdict::Verdict> = if v["last_verdict"].is_null() {
            None
        } else {
            Some(
                serde_json::from_value(v["last_verdict"].clone())
                    .context("pipeline_state.last_verdict is malformed")?,
            )
        };
        let loop_iterations: HashMap<usize, u32> = v["loop_iterations"]
            .as_object()
            .ok_or_else(|| anyhow!("pipeline_state.loop_iterations missing or not an object"))?
            .iter()
            .map(|(k, val)| {
                let ki = k.parse::<usize>().map_err(|_| {
                    anyhow!(
                        "pipeline_state.loop_iterations key {:?} is not a valid index",
                        k
                    )
                })?;
                let vi = val.as_u64().ok_or_else(|| {
                    anyhow!("pipeline_state.loop_iterations[{}] is not a number", k)
                })? as u32;
                Ok((ki, vi))
            })
            .collect::<anyhow::Result<_>>()?;
        // env defaults to empty for backward compat (pre-ADR-0006 files lack this field)
        let env: Vec<(String, String)> = v["env"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .map(|(k, val)| {
                        let s = val
                            .as_str()
                            .ok_or_else(|| anyhow!("pipeline_state.env[{}] is not a string", k))?;
                        Ok((k.clone(), s.to_owned()))
                    })
                    .collect::<anyhow::Result<_>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            current_idx,
            global_counter,
            fail_counts,
            last_stage,
            last_verdict,
            loop_iterations,
            env,
        })
    }
}

/// Build the summary artifact JSON written to `last-run.json`.
pub fn build_summary_artifact(
    summary: &RunSummary,
    workspace_dirty: bool,
    pipeline_state: Option<&PipelineState>,
) -> serde_json::Value {
    let terminal_reason = match summary.terminal_reason {
        TerminalReason::Done => "done",
        TerminalReason::Exit => "exit",
        TerminalReason::FailExit => "fail-exit",
        TerminalReason::CapHit => "cap-hit",
        TerminalReason::Ok => "ok",
    };
    let cap_hit_counter = match &summary.cap_hit {
        None => serde_json::Value::Null,
        Some(CapHitKind::LoopMaxIteration(idx)) => serde_json::json!({
            "type": "max_iteration",
            "loop_idx": idx,
        }),
        Some(CapHitKind::MaxPipelineIterations) => serde_json::json!({
            "type": "max_pipeline_iterations",
        }),
    };
    let last_verdict = summary
        .last_verdict
        .as_ref()
        .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null));
    let loops: serde_json::Map<String, serde_json::Value> = summary
        .iteration_counters
        .loops
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
        .collect();
    let ps = pipeline_state.map(|s| s.to_json());
    serde_json::json!({
        "terminal_reason": terminal_reason,
        "cap_hit_counter": cap_hit_counter,
        "last_stage": summary.last_stage,
        "last_verdict": last_verdict,
        "session_id": summary.session_id,
        "iteration_counters": {
            "global": summary.iteration_counters.global,
            "loops": loops,
        },
        "pipeline_state": ps,
        "timestamp": iso8601_now(),
        "workspace_dirty": workspace_dirty,
    })
}

fn iso8601_now() -> String {
    use std::time::SystemTime;
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let z = secs as i64 / 86400 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let h = (secs / 3600) % 24;
    let min = (secs / 60) % 60;
    let s = secs % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z")
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

    pub fn run(mut self) -> anyhow::Result<PipelineRunResult> {
        let name_to_entry = build_name_index(&self.config);
        let max_pipeline = self.config.max_pipeline_iterations;
        let cap_hit_is_ok = self.config.cap_hit_is_ok;

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

        let capsule_dir = self.capsule_dir.clone();
        let mut early_exit_error: Option<anyhow::Error> = None;

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
                    match run_stage(
                        &mut self.runner,
                        stage,
                        &name_to_entry,
                        &mut progress,
                        capsule_dir.as_deref(),
                    ) {
                        Err(e) => {
                            early_exit_error = Some(e);
                            break (PipelineOutcome::Exit { from_fail: true }, None);
                        }
                        Ok(StageOutcome::Advance(next_idx)) => current_idx = next_idx,
                        Ok(StageOutcome::AdvanceIntoLoop {
                            entry_idx,
                            stage_idx,
                        }) => {
                            let PipelineEntry::Loop(loop_config) = &self.config.entries[entry_idx]
                            else {
                                unreachable!("AdvanceIntoLoop references non-loop entry");
                            };
                            match run_loop(
                                &mut self.runner,
                                loop_config,
                                &mut progress,
                                max_pipeline,
                                stage_idx,
                                capsule_dir.as_deref(),
                            ) {
                                Err(e) => {
                                    early_exit_error = Some(e);
                                    break (PipelineOutcome::Exit { from_fail: true }, None);
                                }
                                Ok(outcome) => {
                                    match handle_loop_outcome(
                                        outcome,
                                        entry_idx,
                                        &mut loop_iterations,
                                    ) {
                                        LoopControl::Advance(next) => current_idx = next,
                                        LoopControl::Break(o, cap) => break (o, cap),
                                    }
                                }
                            }
                        }
                        Ok(StageOutcome::Done) => break (PipelineOutcome::Done, None),
                        Ok(StageOutcome::Exit(ExitKind::PassRoute)) => {
                            break (PipelineOutcome::Exit { from_fail: false }, None)
                        }
                        Ok(StageOutcome::Exit(ExitKind::FailRoute)) => {
                            break (PipelineOutcome::Exit { from_fail: true }, None)
                        }
                    }
                }
                PipelineEntry::Loop(loop_config) => {
                    let entry_idx = current_idx;
                    match run_loop(
                        &mut self.runner,
                        loop_config,
                        &mut progress,
                        max_pipeline,
                        0,
                        capsule_dir.as_deref(),
                    ) {
                        Err(e) => {
                            early_exit_error = Some(e);
                            break (PipelineOutcome::Exit { from_fail: true }, None);
                        }
                        Ok(outcome) => {
                            match handle_loop_outcome(outcome, entry_idx, &mut loop_iterations) {
                                LoopControl::Advance(next) => current_idx = next,
                                LoopControl::Break(o, cap) => break (o, cap),
                            }
                        }
                    }
                }
            }
        };

        if let Some(e) = early_exit_error {
            return Err(e);
        }

        let terminal_reason = match (&outcome, cap_hit_is_ok) {
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
            env: vec![],
        };

        Ok(PipelineRunResult {
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
        })
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

enum RouteTarget {
    Entry(usize),
    LoopStage { entry_idx: usize, stage_idx: usize },
}

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
    capsule_dir: Option<&Path>,
) -> anyhow::Result<LoopOutcome> {
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
                    return Ok(LoopOutcome::CapHit {
                        kind: LoopCapKind::MaxIteration,
                        iterations: iteration_count - 1,
                    });
                }
            }
        }
        retrying_top = false;

        if progress.global_counter >= max_pipeline {
            return Ok(LoopOutcome::CapHit {
                kind: LoopCapKind::MaxPipelineIterations,
                iterations: iteration_count,
            });
        }
        progress.global_counter += 1;

        let stage = &loop_config.stages[stage_idx];
        let base_prompt = resolve_stage_prompt(stage, capsule_dir)?;
        let with_input = inject_input(&mut progress.input, &base_prompt);
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
            return Ok(LoopOutcome::LoopDone {
                iterations: iteration_count,
            });
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
                        return Ok(LoopOutcome::Exit {
                            kind: ExitKind::PassRoute,
                            iterations: iteration_count,
                        })
                    }
                },
                OnPass::Exit => {
                    return Ok(LoopOutcome::Exit {
                        kind: ExitKind::PassRoute,
                        iterations: iteration_count,
                    })
                }
            }
        } else {
            let fail_count = progress.fail_counts.entry(stage.name.clone()).or_insert(0);
            *fail_count += 1;

            if let Some(max) = stage.max_retries {
                if *fail_count > max {
                    return Ok(LoopOutcome::Exit {
                        kind: ExitKind::FailRoute,
                        iterations: iteration_count,
                    });
                }
            }

            match &stage.on_fail {
                OnFail::Exit => {
                    return Ok(LoopOutcome::Exit {
                        kind: ExitKind::FailRoute,
                        iterations: iteration_count,
                    })
                }
                OnFail::Retry => {
                    if stage_idx == 0 {
                        retrying_top = true;
                    }
                }
                OnFail::Stage(name) => match loop_name_to_idx.get(name.as_str()) {
                    Some(&idx) => stage_idx = idx,
                    None => {
                        return Ok(LoopOutcome::Exit {
                            kind: ExitKind::FailRoute,
                            iterations: iteration_count,
                        })
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

fn inject_input(input: &mut Option<String>, base_prompt: &str) -> String {
    if let Some(text) = input.take() {
        format!("<capsule:input>\n{text}\n</capsule:input>\n\n{base_prompt}")
    } else {
        base_prompt.to_string()
    }
}

fn resolve_stage_prompt(stage: &StageConfig, capsule_dir: Option<&Path>) -> anyhow::Result<String> {
    match (stage.prompt.as_deref(), capsule_dir) {
        (Some(path_str), Some(dir)) => {
            let content = read_prompt_file(dir, path_str)
                .with_context(|| format!("stage '{}': failed to load prompt", stage.name))?;
            Ok(prepend_preamble(&content))
        }
        (Some(literal), None) => Ok(literal.to_string()),
        (None, _) => Ok(String::new()),
    }
}

fn inject_note_block(
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

fn run_stage(
    runner: &mut dyn StageRunner,
    stage: &StageConfig,
    name_to_entry: &HashMap<String, RouteTarget>,
    progress: &mut PipelineProgress,
    capsule_dir: Option<&Path>,
) -> anyhow::Result<StageOutcome> {
    let base_prompt = resolve_stage_prompt(stage, capsule_dir)?;
    let with_input = inject_input(&mut progress.input, &base_prompt);
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
        return Ok(StageOutcome::Done);
    }

    let is_pass = matches!(
        verdict.as_ref().map(|v| &v.status),
        Some(VerdictStatus::Pass)
    );

    if is_pass {
        progress.fail_counts.remove(&stage.name);
        Ok(route_pass(stage, name_to_entry))
    } else {
        let fail_count = progress.fail_counts.entry(stage.name.clone()).or_insert(0);
        *fail_count += 1;

        if let Some(max) = stage.max_retries {
            if *fail_count > max {
                return Ok(StageOutcome::Exit(ExitKind::FailRoute));
            }
        }

        Ok(route_fail(stage, name_to_entry))
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
            cap_hit_is_ok: false,
        }
    }

    fn single_stage_entry(s: StageConfig) -> PipelineEntry {
        PipelineEntry::Stage(s)
    }

    fn run_outcome(config: PipelineConfig, runner: FakeRunner) -> PipelineOutcome {
        PipelineExecutor::new(config, runner).run().unwrap().outcome
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
        s.max_retries = Some(2);
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

    #[test]
    fn max_pipeline_iterations_caps_total() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_pipeline_iterations: 3,
            cap_hit_is_ok: false,
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
            PipelineExecutor::new(config, runner).run().unwrap().outcome,
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
        ) -> Option<Verdict> {
            self.prompts.lock().unwrap().push(prompt.to_string());
            self.responses
                .pop_front()
                .expect("RecordingRunner: no more responses queued")
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
        PipelineExecutor::new(config, runner).run().unwrap().summary
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
            cap_hit_is_ok: true,
        };
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
        assert_eq!(summary.cap_hit, Some(CapHitKind::LoopMaxIteration(0)));
    }

    #[test]
    fn cap_hit_identifies_global_when_max_pipeline_exceeded() {
        let mut s = stage("a");
        s.on_fail = OnFail::Retry;
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_pipeline_iterations: 2,
            cap_hit_is_ok: false,
        };
        let summary = run_summary(config, FakeRunner::new([fail(), fail(), fail()]));
        assert_eq!(summary.cap_hit, Some(CapHitKind::MaxPipelineIterations));
    }

    fn run_result(config: PipelineConfig, runner: FakeRunner) -> PipelineRunResult {
        PipelineExecutor::new(config, runner).run().unwrap()
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
        let result = PipelineExecutor::resume(config, FakeRunner::new([pass(), pass()]), state)
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

        let first_run = PipelineExecutor::new(config.clone(), FakeRunner::new([fail()]))
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
        let config = PipelineConfig {
            entries: vec![single_stage_entry(s)],
            max_pipeline_iterations: 5,
            cap_hit_is_ok: false,
        };
        // First run: 3 fails → CapHit at max 3
        let config3 = PipelineConfig {
            entries: config.entries.clone(),
            max_pipeline_iterations: 3,
            cap_hit_is_ok: false,
        };
        let first_run = run_result(config3, FakeRunner::new([fail(), fail(), fail()]));
        assert_eq!(first_run.pipeline_state.global_counter, 3);

        // Resume with higher limit: counter starts at 3, 2 more iterations available
        let state = first_run.pipeline_state;
        let result = PipelineExecutor::resume(config, FakeRunner::new([fail(), pass()]), state)
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
        let mut fail_counts = HashMap::new();
        fail_counts.insert("stage-a".to_string(), 2u32);
        let mut loop_iters = HashMap::new();
        loop_iters.insert(0usize, 3u32);
        PipelineState {
            current_idx: 2,
            global_counter: 7,
            fail_counts,
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
        let v = state.to_json();
        assert_eq!(v["current_idx"], 2);
        assert_eq!(v["global_counter"], 7);
        assert_eq!(v["fail_counts"]["stage-a"], 2);
        assert_eq!(v["last_stage"], "stage-a");
        assert_eq!(v["last_verdict"]["status"], "fail");
        assert_eq!(v["last_verdict"]["notes"], "oops");
        assert_eq!(v["loop_iterations"]["0"], 3);
        assert_eq!(v["env"]["KEY"], "val");
    }

    #[test]
    fn pipeline_state_round_trips_via_json() {
        let original = full_state();
        let json = original.to_json();
        let restored = PipelineState::from_json(&json).unwrap();
        assert_eq!(restored.current_idx, original.current_idx);
        assert_eq!(restored.global_counter, original.global_counter);
        assert_eq!(restored.fail_counts, original.fail_counts);
        assert_eq!(restored.last_stage, original.last_stage);
        assert_eq!(restored.last_verdict, original.last_verdict);
        assert_eq!(restored.loop_iterations, original.loop_iterations);
        assert_eq!(restored.env, original.env);
    }

    #[test]
    fn from_json_errors_on_missing_current_idx() {
        let mut v = full_state().to_json();
        v.as_object_mut().unwrap().remove("current_idx");
        assert!(PipelineState::from_json(&v).is_err());
    }

    #[test]
    fn from_json_errors_on_missing_global_counter() {
        let mut v = full_state().to_json();
        v.as_object_mut().unwrap().remove("global_counter");
        assert!(PipelineState::from_json(&v).is_err());
    }

    #[test]
    fn from_json_errors_on_missing_fail_counts() {
        let mut v = full_state().to_json();
        v.as_object_mut().unwrap().remove("fail_counts");
        assert!(PipelineState::from_json(&v).is_err());
    }

    #[test]
    fn from_json_errors_on_non_number_in_fail_counts() {
        let mut v = full_state().to_json();
        v["fail_counts"]["stage-a"] = serde_json::json!("not-a-number");
        assert!(PipelineState::from_json(&v).is_err());
    }

    #[test]
    fn from_json_errors_on_missing_loop_iterations() {
        let mut v = full_state().to_json();
        v.as_object_mut().unwrap().remove("loop_iterations");
        assert!(PipelineState::from_json(&v).is_err());
    }

    #[test]
    fn from_json_errors_on_non_number_in_loop_iterations() {
        let mut v = full_state().to_json();
        v["loop_iterations"]["0"] = serde_json::json!("not-a-number");
        assert!(PipelineState::from_json(&v).is_err());
    }

    #[test]
    fn from_json_errors_on_malformed_last_verdict() {
        let mut v = full_state().to_json();
        v["last_verdict"] = serde_json::json!({"status": 42});
        assert!(PipelineState::from_json(&v).is_err());
    }

    #[test]
    fn from_json_env_defaults_to_empty_when_absent() {
        let mut v = full_state().to_json();
        v.as_object_mut().unwrap().remove("env");
        let restored = PipelineState::from_json(&v).unwrap();
        assert_eq!(restored.env, vec![]);
    }

    #[test]
    fn from_json_errors_on_non_string_in_env() {
        let mut v = full_state().to_json();
        v["env"]["KEY"] = serde_json::json!(42);
        assert!(PipelineState::from_json(&v).is_err());
    }
}
