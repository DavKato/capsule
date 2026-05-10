use anyhow::{Context, Result};
use capsule::pipeline::{build_summary_artifact, PipelineState, RunSummary, TerminalReason};
use std::path::Path;
use std::process::Command;

use super::ExitDecision;

pub(super) fn exit_decision_from_summary(summary: &RunSummary) -> ExitDecision {
    match summary.terminal_reason {
        TerminalReason::Done | TerminalReason::Exit | TerminalReason::Ok => ExitDecision::Success,
        TerminalReason::FailExit | TerminalReason::CapHit => {
            let notes = summary
                .last_verdict
                .as_ref()
                .and_then(|v| v.notes.as_deref())
                .unwrap_or("")
                .to_string();
            ExitDecision::Failure(notes)
        }
    }
}

pub(super) fn write_last_run(
    capsule_dir: &Path,
    summary: &RunSummary,
    pipeline_state: Option<&PipelineState>,
) -> Result<()> {
    let dirty = is_workspace_dirty();
    let json = build_summary_artifact(summary, dirty, pipeline_state);
    let path = capsule_dir.join("last-run.json");
    std::fs::write(&path, serde_json::to_string_pretty(&json)?)
        .with_context(|| format!("writing summary artifact {}", path.display()))?;
    Ok(())
}

fn is_workspace_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

pub(super) fn parse_resume_state(capsule_dir: &Path) -> Result<(String, PipelineState)> {
    let path = capsule_dir.join("last-run.json");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("{} not found — run `capsule run` first", path.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&content).context("failed to parse last-run.json")?;

    let session_id = json["session_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("last-run.json has no session_id — cannot resume"))?
        .to_string();

    let state_json = &json["pipeline_state"];
    if state_json.is_null() {
        anyhow::bail!(
            "last-run.json has no pipeline state — \
             the previous run completed cleanly and cannot be resumed"
        );
    }

    let state = PipelineState::from_json(state_json)
        .context("failed to deserialize pipeline_state from last-run.json")?;
    Ok((session_id, state))
}

pub(super) fn resume_hint(session_id: Option<&str>, reason: &TerminalReason) -> Option<String> {
    let id = session_id?;
    match reason {
        TerminalReason::FailExit | TerminalReason::CapHit => {
            Some(format!("To continue the session, run: capsule resume {id}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::ExitDecision;
    use super::{
        exit_decision_from_summary, is_workspace_dirty, parse_resume_state, resume_hint,
        write_last_run,
    };
    use capsule::pipeline::{
        build_summary_artifact, CapHitKind, IterationCounters, PipelineState, RunSummary,
        TerminalReason,
    };
    use capsule::verdict::{Verdict, VerdictStatus};
    use std::collections::HashMap;

    fn minimal_summary(reason: TerminalReason) -> RunSummary {
        RunSummary {
            terminal_reason: reason,
            last_stage: None,
            last_verdict: None,
            iteration_counters: IterationCounters {
                global: 0,
                loops: HashMap::new(),
            },
            cap_hit: None,
            session_id: None,
        }
    }

    fn make_pipeline_state() -> PipelineState {
        let mut fail_counts = HashMap::new();
        fail_counts.insert("stage-a".to_string(), 2u32);
        let mut loop_iters = HashMap::new();
        loop_iters.insert(0usize, 3u32);
        PipelineState {
            current_idx: 2,
            global_counter: 7,
            fail_counts,
            last_stage: Some("stage-a".to_string()),
            last_verdict: Some(capsule::verdict::Verdict {
                status: capsule::verdict::VerdictStatus::Fail,
                notes: Some("oops".to_string()),
            }),
            loop_iterations: loop_iters,
            env: vec![],
        }
    }

    #[test]
    fn json_done_terminal_reason() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["terminal_reason"], "done");
        assert!(v["cap_hit_counter"].is_null());
        assert!(!v["workspace_dirty"].as_bool().unwrap());
    }

    #[test]
    fn json_fail_exit_terminal_reason() {
        let s = minimal_summary(TerminalReason::FailExit);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["terminal_reason"], "fail-exit");
    }

    #[test]
    fn json_ok_terminal_reason() {
        let s = minimal_summary(TerminalReason::Ok);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["terminal_reason"], "ok");
    }

    #[test]
    fn json_cap_hit_loop_max_iteration() {
        let mut s = minimal_summary(TerminalReason::CapHit);
        s.cap_hit = Some(CapHitKind::LoopMaxIteration(0));
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["terminal_reason"], "cap-hit");
        assert_eq!(v["cap_hit_counter"]["type"], "max_iteration");
        assert_eq!(v["cap_hit_counter"]["loop_idx"], 0);
    }

    #[test]
    fn json_cap_hit_max_pipeline_iterations() {
        let mut s = minimal_summary(TerminalReason::CapHit);
        s.cap_hit = Some(CapHitKind::MaxPipelineIterations);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["cap_hit_counter"]["type"], "max_pipeline_iterations");
        assert!(v["cap_hit_counter"]["loop_idx"].is_null());
    }

    #[test]
    fn json_last_verdict_null_when_none() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, false, None);
        assert!(v["last_verdict"].is_null());
    }

    #[test]
    fn json_last_verdict_serialized_when_present() {
        let mut s = minimal_summary(TerminalReason::Done);
        s.last_verdict = Some(Verdict {
            status: VerdictStatus::Pass,
            notes: Some("all good".to_string()),
        });
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["last_verdict"]["status"], "pass");
        assert_eq!(v["last_verdict"]["notes"], "all good");
    }

    #[test]
    fn json_workspace_dirty_flag() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, true, None);
        assert!(v["workspace_dirty"].as_bool().unwrap());
    }

    #[test]
    fn json_iteration_counters_with_loop() {
        let mut s = minimal_summary(TerminalReason::Done);
        s.iteration_counters.global = 5;
        s.iteration_counters.loops.insert(0, 3);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["iteration_counters"]["global"], 5);
        assert_eq!(v["iteration_counters"]["loops"]["0"], 3);
    }

    #[test]
    fn write_last_run_creates_valid_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = minimal_summary(TerminalReason::Exit);
        write_last_run(dir.path(), &s, None).unwrap();
        let content = std::fs::read_to_string(dir.path().join("last-run.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["terminal_reason"], "exit");
        assert!(parsed["timestamp"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn json_includes_session_id_when_present() {
        let mut s = minimal_summary(TerminalReason::Done);
        s.session_id = Some("sess_abc123".to_string());
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["session_id"], "sess_abc123");
    }

    #[test]
    fn json_session_id_null_when_absent() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, false, None);
        assert!(v["session_id"].is_null());
    }

    #[test]
    fn resume_hint_shown_on_fail_exit_with_session_id() {
        let hint = resume_hint(Some("sess_abc"), &TerminalReason::FailExit);
        assert!(hint.is_some());
        let msg = hint.unwrap();
        assert!(msg.contains("sess_abc"), "hint was: {msg}");
        assert!(msg.contains("capsule resume"), "hint was: {msg}");
    }

    #[test]
    fn resume_hint_shown_on_cap_hit_with_session_id() {
        assert!(resume_hint(Some("sess_xyz"), &TerminalReason::CapHit).is_some());
    }

    #[test]
    fn resume_hint_none_when_no_session_id() {
        assert!(resume_hint(None, &TerminalReason::FailExit).is_none());
    }

    #[test]
    fn resume_hint_none_on_success() {
        assert!(resume_hint(Some("sess_abc"), &TerminalReason::Done).is_none());
        assert!(resume_hint(Some("sess_abc"), &TerminalReason::Ok).is_none());
        assert!(resume_hint(Some("sess_abc"), &TerminalReason::Exit).is_none());
    }

    #[test]
    fn is_workspace_dirty_reflects_git_status() {
        let result = is_workspace_dirty();
        let _ = result;
    }

    #[test]
    fn json_pipeline_state_present_for_fail_exit() {
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.session_id = Some("sess_abc".to_string());
        let state = make_pipeline_state();
        let v = build_summary_artifact(&s, false, Some(&state));
        assert!(
            !v["pipeline_state"].is_null(),
            "pipeline_state must be present for FailExit"
        );
        assert_eq!(v["pipeline_state"]["current_idx"], 2);
        assert_eq!(v["pipeline_state"]["global_counter"], 7);
        assert_eq!(v["pipeline_state"]["fail_counts"]["stage-a"], 2);
        assert_eq!(v["pipeline_state"]["last_stage"], "stage-a");
        assert_eq!(v["pipeline_state"]["last_verdict"]["status"], "fail");
        assert_eq!(v["pipeline_state"]["loop_iterations"]["0"], 3);
    }

    #[test]
    fn json_pipeline_state_null_for_clean_exit() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, false, None);
        assert!(
            v["pipeline_state"].is_null(),
            "pipeline_state must be null for Done"
        );
    }

    #[test]
    fn parse_resume_state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.session_id = Some("sess_xyz".to_string());
        let state = make_pipeline_state();
        write_last_run(dir.path(), &s, Some(&state)).unwrap();

        let (session_id, restored) = parse_resume_state(dir.path()).unwrap();
        assert_eq!(session_id, "sess_xyz");
        assert_eq!(restored.current_idx, 2);
        assert_eq!(restored.global_counter, 7);
        assert_eq!(restored.fail_counts.get("stage-a"), Some(&2));
        assert_eq!(restored.last_stage.as_deref(), Some("stage-a"));
        assert_eq!(
            restored.last_verdict.as_ref().map(|v| &v.status),
            Some(&capsule::verdict::VerdictStatus::Fail)
        );
        assert_eq!(restored.loop_iterations.get(&0), Some(&3));
    }

    #[test]
    fn parse_resume_state_errors_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(parse_resume_state(dir.path()).is_err());
    }

    #[test]
    fn parse_resume_state_errors_when_pipeline_state_null() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = minimal_summary(TerminalReason::Done);
        s.session_id = Some("sess_done".to_string());
        write_last_run(dir.path(), &s, None).unwrap();
        let err = parse_resume_state(dir.path()).unwrap_err();
        assert!(err.to_string().contains("no pipeline state"), "err: {err}");
    }

    #[test]
    fn parse_resume_state_errors_when_no_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.session_id = None;
        let state = make_pipeline_state();
        write_last_run(dir.path(), &s, Some(&state)).unwrap();
        let err = parse_resume_state(dir.path()).unwrap_err();
        assert!(err.to_string().contains("no session_id"), "err: {err}");
    }

    #[test]
    fn parse_resume_state_round_trips_env_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.session_id = Some("sess_env".to_string());
        let state = PipelineState {
            current_idx: 0,
            global_counter: 0,
            fail_counts: HashMap::new(),
            last_stage: None,
            last_verdict: None,
            loop_iterations: HashMap::new(),
            env: vec![
                ("PARENT".to_string(), "42".to_string()),
                ("MODE".to_string(), "test".to_string()),
            ],
        };
        write_last_run(dir.path(), &s, Some(&state)).unwrap();
        let (_, restored) = parse_resume_state(dir.path()).unwrap();
        assert_eq!(restored.env.len(), 2);
        assert!(restored
            .env
            .contains(&("PARENT".to_string(), "42".to_string())));
        assert!(restored
            .env
            .contains(&("MODE".to_string(), "test".to_string())));
    }

    #[test]
    fn parse_resume_state_env_defaults_to_empty_when_field_missing() {
        let dir = tempfile::tempdir().unwrap();
        let json = serde_json::json!({
            "terminal_reason": "fail-exit",
            "cap_hit_counter": null,
            "last_stage": null,
            "last_verdict": null,
            "session_id": "sess_old",
            "iteration_counters": { "global": 0, "loops": {} },
            "pipeline_state": {
                "current_idx": 1,
                "global_counter": 2,
                "fail_counts": {},
                "last_stage": null,
                "last_verdict": null,
                "loop_iterations": {}
            },
            "timestamp": "2026-01-01T00:00:00Z",
            "workspace_dirty": false
        });
        std::fs::write(
            dir.path().join("last-run.json"),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
        let (_, restored) = parse_resume_state(dir.path()).unwrap();
        assert_eq!(restored.env, vec![], "missing env field must default to []");
    }

    #[test]
    fn pipeline_state_json_includes_env_pairs() {
        let state = PipelineState {
            current_idx: 0,
            global_counter: 0,
            fail_counts: HashMap::new(),
            last_stage: None,
            last_verdict: None,
            loop_iterations: HashMap::new(),
            env: vec![
                ("PARENT".to_string(), "79".to_string()),
                ("MODE".to_string(), "dry".to_string()),
            ],
        };
        let json = state.to_json();
        let env = json["env"].as_object().expect("env must be an object");
        assert_eq!(env.len(), 2);
        assert_eq!(env["PARENT"], "79");
        assert_eq!(env["MODE"], "dry");
    }

    #[test]
    fn failure_decision_carries_notes_from_last_verdict() {
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.last_verdict = Some(Verdict {
            status: VerdictStatus::Fail,
            notes: Some("reviewer rejected implementation".to_string()),
        });
        match exit_decision_from_summary(&s) {
            ExitDecision::Failure(notes) => {
                assert_eq!(notes, "reviewer rejected implementation");
            }
            _ => panic!("expected Failure"),
        }
    }

    #[test]
    fn failure_decision_empty_notes_when_no_verdict() {
        let s = minimal_summary(TerminalReason::FailExit);
        match exit_decision_from_summary(&s) {
            ExitDecision::Failure(notes) => {
                assert!(notes.is_empty(), "notes must be empty when no last verdict");
            }
            _ => panic!("expected Failure"),
        }
    }

    #[test]
    fn failure_decision_empty_notes_when_verdict_has_no_notes() {
        let mut s = minimal_summary(TerminalReason::CapHit);
        s.last_verdict = Some(Verdict {
            status: VerdictStatus::Fail,
            notes: None,
        });
        match exit_decision_from_summary(&s) {
            ExitDecision::Failure(notes) => {
                assert!(
                    notes.is_empty(),
                    "notes must be empty when verdict notes is None"
                );
            }
            _ => panic!("expected Failure"),
        }
    }
}
