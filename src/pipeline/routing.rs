use std::collections::HashMap;

use super::RetryInfo;
use crate::config::{LoopConfig, OnFail, OnPass, StageConfig};
use crate::verdict::{Verdict, VerdictStatus};

use super::prompt::{inject_input, inject_note_block};
use super::summary::{CapHitKind, PipelineOutcome};
use super::StageRunner;

fn retry_info(progress: &PipelineProgress, stage: &StageConfig) -> Option<RetryInfo> {
    let fail_count = progress.fail_counts.get(&stage.name).copied().unwrap_or(0);
    if fail_count > 0 {
        Some(RetryInfo {
            current: fail_count,
            max: stage.max_retries,
        })
    } else {
        None
    }
}

/// Mutable progress tracked across stage invocations during a pipeline run.
pub(super) struct PipelineProgress {
    pub(super) fail_counts: HashMap<String, u32>,
    pub(super) global_counter: u32,
    pub(super) input: Option<String>,
    pub(super) last_stage: Option<String>,
    pub(super) last_verdict: Option<Verdict>,
}

#[derive(Clone)]
pub(super) enum ExitKind {
    PassRoute,
    FailRoute,
}

pub(super) enum LoopCapKind {
    MaxIteration,
    MaxStages,
}

pub(super) enum StageOutcome {
    Advance(usize),
    AdvanceIntoLoop { entry_idx: usize, stage_idx: usize },
    Done,
    Exit(ExitKind),
}

pub(super) enum RouteTarget {
    Entry(usize),
    LoopStage { entry_idx: usize, stage_idx: usize },
}

pub(super) enum LoopControl {
    Advance(usize),
    Break(PipelineOutcome, Option<CapHitKind>),
}

pub(super) enum LoopOutcome {
    LoopDone { iterations: u32 },
    Exit { kind: ExitKind, iterations: u32 },
    CapHit { kind: LoopCapKind, iterations: u32 },
}

pub(super) fn run_loop(
    runner: &mut dyn StageRunner,
    loop_config: &LoopConfig,
    progress: &mut PipelineProgress,
    max_stages: u32,
    start_stage_idx: usize,
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

        if progress.global_counter >= max_stages {
            return Ok(LoopOutcome::CapHit {
                kind: LoopCapKind::MaxStages,
                iterations: iteration_count,
            });
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
        let retry = retry_info(progress, stage);
        let verdict = runner.run(
            &stage.name,
            &effective_prompt,
            stage.model.as_deref(),
            stage.setup.as_deref(),
            retry.as_ref(),
        )?;
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

            if *fail_count > stage.max_retries {
                return Ok(LoopOutcome::Exit {
                    kind: ExitKind::FailRoute,
                    iterations: iteration_count,
                });
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
pub(super) fn build_name_index(
    config: &crate::config::PipelineConfig,
) -> HashMap<String, RouteTarget> {
    let mut map = HashMap::new();
    for (i, entry) in config.entries.iter().enumerate() {
        match entry {
            crate::config::PipelineEntry::Stage(s) => {
                map.insert(s.name.clone(), RouteTarget::Entry(i));
            }
            crate::config::PipelineEntry::Loop(l) => {
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

pub(super) fn handle_loop_outcome(
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
            kind: LoopCapKind::MaxStages,
            iterations,
        } => {
            loop_iterations.insert(entry_idx, iterations);
            LoopControl::Break(PipelineOutcome::CapHit, Some(CapHitKind::MaxStages))
        }
    }
}

pub(super) fn run_stage(
    runner: &mut dyn StageRunner,
    stage: &StageConfig,
    name_to_entry: &HashMap<String, RouteTarget>,
    progress: &mut PipelineProgress,
) -> anyhow::Result<StageOutcome> {
    let base_prompt = stage.prompt.as_deref().unwrap_or("");
    let with_input = inject_input(&mut progress.input, base_prompt);
    let effective_prompt = inject_note_block(
        progress.last_stage.as_deref(),
        &progress.last_verdict,
        &with_input,
    );
    progress.last_stage = Some(stage.name.clone());
    let retry = retry_info(progress, stage);
    let verdict = runner.run(
        &stage.name,
        &effective_prompt,
        stage.model.as_deref(),
        stage.setup.as_deref(),
        retry.as_ref(),
    )?;
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

        if *fail_count > stage.max_retries {
            return Ok(StageOutcome::Exit(ExitKind::FailRoute));
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
