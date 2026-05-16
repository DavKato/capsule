use std::collections::HashMap;

use crate::verdict::Verdict;

use super::state::PipelineState;

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
    /// The global `max_stages` was exceeded.
    MaxStages,
}

/// Snapshot of iteration counters at pipeline exit.
#[derive(Debug, Clone, PartialEq)]
pub struct IterationCounters {
    pub global: u32,
    /// Per-loop iteration counts, keyed by the loop's index in the top-level entries list.
    pub loops: HashMap<usize, u32>,
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
        Some(CapHitKind::MaxStages) => serde_json::json!({
            "type": "max_stages",
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
    let ps = pipeline_state.map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null));
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

pub(super) fn iso8601_now() -> String {
    use std::time::SystemTime;
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    iso8601_from_secs(secs)
}

pub(super) fn iso8601_from_secs(secs: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_epoch_zero() {
        assert_eq!(iso8601_from_secs(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_known_date() {
        // 2025-01-01T00:00:00Z = 1735689600
        assert_eq!(iso8601_from_secs(1_735_689_600), "2025-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_mid_day() {
        assert_eq!(iso8601_from_secs(961_073_130), "2000-06-15T12:45:30Z");
    }
}
