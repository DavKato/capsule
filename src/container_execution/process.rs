use super::docker_args::{build_docker_args, container_name_for};
use super::infra::{host_token_is_expired, make_mcp_config};
use super::stream_parser::{ModelUsage, StreamParser, TextDisplay, ToolEvent, UsageSnapshot};
use super::{ExecutionConfig, IterationOutcome};
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

pub struct StreamResult {
    pub auth_failed: bool,
    pub submit_verdict_missing: bool,
    pub verdict: Option<crate::verdict::Verdict>,
    pub session_id: Option<String>,
    pub model_usage: Option<ModelUsage>,
    pub last_usage_snapshot: Option<UsageSnapshot>,
}

pub fn post_stream_error(
    result: &StreamResult,
    status: &std::process::ExitStatus,
    context: &str,
) -> Option<anyhow::Error> {
    if result.auth_failed {
        return Some(anyhow::anyhow!(
            "Claude authentication failed on {context}. \
             Run `claude auth login` on the host to refresh credentials, then retry."
        ));
    }
    if result.submit_verdict_missing {
        return Some(anyhow::anyhow!(
            "The `submit_verdict` MCP tool was not registered. \
             Likely causes: the base image is stale (run `capsule run --rebuild` to force a rebuild), \
             the capsule binary is not on PATH inside the container, \
             or `.mcp.json` was not mounted."
        ));
    }
    if !status.success() {
        return Some(anyhow::anyhow!(
            "container exited with code {} during {context}",
            status.code().unwrap_or(-1)
        ));
    }
    None
}

fn stream_output(reader: BufReader<impl std::io::Read>, verbose: bool) -> Result<StreamResult> {
    let mut parser = StreamParser::new();
    let mut last_snapshot: Option<UsageSnapshot> = None;

    for line in reader.lines() {
        let line = line.context("error reading docker stdout")?;
        let verdict_seen = parser.feed(&line).is_some();
        if verbose {
            if let Some(v) = parser.last_parsed_value() {
                let pretty = serde_json::to_string_pretty(v).unwrap_or_else(|_| line.clone());
                crate::display::info(&pretty);
            } else {
                crate::display::info(&line);
            }
        }
        let had_tool_events = !parser.last_tool_events().is_empty();
        for event in parser.last_tool_events() {
            match event {
                ToolEvent::Use(tu) => {
                    let args = format_tool_args(&tu.input);
                    crate::display::tool_call(&tu.name, &args, &tu.id);
                }
                ToolEvent::Result(tr) => {
                    crate::display::tool_result(&tr.tool_use_id, !tr.is_error);
                }
            }
        }
        let had_text = !parser.last_text_displays().is_empty();
        for display in parser.last_text_displays() {
            let text = match display {
                TextDisplay::Content(t) | TextDisplay::Thinking(t) => t,
            };
            crate::display::agent_text(text);
        }
        if let Some(snap) = parser.last_usage_snapshot() {
            crate::display::set_usage(snap.total_tokens());
            last_snapshot = Some(snap.clone());
        }
        if !had_text
            && !had_tool_events
            && !verdict_seen
            && !line.is_empty()
            && parser.last_parsed_value().is_none()
        {
            crate::display::info(&line);
        }
    }

    Ok(StreamResult {
        auth_failed: parser.auth_failed(),
        submit_verdict_missing: parser.submit_verdict_missing(),
        verdict: parser.verdict().cloned(),
        session_id: parser.session_id().map(str::to_owned),
        model_usage: parser.model_usage().cloned(),
        last_usage_snapshot: last_snapshot,
    })
}

fn format_tool_args(input: &serde_json::Value) -> String {
    let Some(obj) = input.as_object() else {
        return String::new();
    };
    for key in &["command", "file_path", "path", "pattern", "prompt"] {
        if let Some(s) = obj.get(*key).and_then(serde_json::Value::as_str) {
            return s.replace('\n', " ");
        }
    }
    for val in obj.values() {
        if let Some(s) = val.as_str() {
            return s.replace('\n', " ");
        }
    }
    String::new()
}

/// Shared scaffolding for one container run: temp files, docker args, MCP config,
/// streaming, wait, and active-container slot management.
///
/// `resume_session_id` — when `Some`, adds `-e=CAPSULE_RESUME_SESSION=<id>` so the
/// entrypoint invokes `claude --resume` instead of piping `prompt.txt`.
///
/// Returns the parsed stream result and the container exit status. Callers own all
/// post-stream policy (retry, bail, verdict routing).
pub fn run_container(
    cfg: &ExecutionConfig,
    container_name: &str,
    active_container: &Arc<Mutex<Option<String>>>,
    resume_session_id: Option<&str>,
) -> Result<(StreamResult, std::process::ExitStatus)> {
    let mut prompt_file = tempfile::Builder::new()
        .prefix("capsule-prompt-")
        .suffix(".txt")
        .tempfile()
        .context("failed to create prompt temp file")?;
    prompt_file
        .write_all(cfg.prompt.as_bytes())
        .context("failed to write prompt to temp file")?;
    prompt_file.flush().context("failed to flush prompt file")?;
    let prompt_path = prompt_file.path().to_owned();

    const CAPSULE_CONTAINER_BIN: &str = "/usr/local/bin/capsule";
    let mcp_config = make_mcp_config(std::path::Path::new(CAPSULE_CONTAINER_BIN));
    let mut mcp_file = tempfile::Builder::new()
        .prefix("capsule-mcp-")
        .suffix(".json")
        .tempfile()
        .context("failed to create mcp config temp file")?;
    mcp_file
        .write_all(mcp_config.as_bytes())
        .context("failed to write mcp config")?;
    mcp_file.flush().context("failed to flush mcp config")?;
    let mcp_path = mcp_file.path().to_owned();

    let capsule_host_bin =
        std::env::current_exe().context("failed to resolve capsule binary path")?;

    if let Ok(mut slot) = active_container.lock() {
        *slot = Some(container_name.to_string());
    }

    crate::dev::log_docker_env(
        cfg.env_file.as_deref(),
        cfg.extra_env_file.as_deref(),
        container_name,
    );

    let mut docker_args = build_docker_args(cfg, &prompt_path, container_name);
    let image = docker_args
        .pop()
        .expect("docker args must end with image name");
    docker_args.push(format!(
        "--mount=type=bind,src={},dst={},readonly",
        capsule_host_bin.display(),
        CAPSULE_CONTAINER_BIN
    ));
    docker_args.push(format!(
        "--mount=type=bind,src={},dst=/home/claude/.mcp.json,readonly",
        mcp_path.display()
    ));
    if let Some(sid) = resume_session_id {
        docker_args.push(format!("-e=CAPSULE_RESUME_SESSION={sid}"));
    }
    docker_args.push(image);

    let mut docker_child = Command::new("docker")
        .args(&docker_args)
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn `docker run`")?;

    let reader = BufReader::new(docker_child.stdout.take().expect("stdout piped"));
    let result = stream_output(reader, cfg.verbose)?;

    let status = docker_child.wait().context("docker run did not complete")?;

    if let Ok(mut slot) = active_container.lock() {
        *slot = None;
    }

    Ok((result, status))
}

/// Returns true when an auth failure can be retried via `--resume`.
///
/// All three conditions must hold: there is a captured session ID to resume,
/// a credentials file to re-copy, and the host token has not already expired.
fn should_attempt_resume(
    session_id: Option<&str>,
    has_credentials: bool,
    host_token_expired: bool,
) -> bool {
    session_id.is_some() && has_credentials && !host_token_expired
}

/// Run one iteration: mount prompt, stream output, propagate exit code.
///
/// `iteration` is used to derive a unique `--name` for the container so that a
/// registered ctrlc handler can call `docker stop <name>` on SIGINT.
/// `active_container` is a shared slot; this function writes the container name
/// before spawning and clears it after the container exits.
///
/// Returns [`IterationOutcome::AuthFailedResumable`] when auth failed but the host
/// token is still valid — the caller is responsible for re-copying credentials,
/// resetting the guard baseline, and launching the resume container.
///
/// # Errors
/// - Container exits non-zero → error naming the exit code.
/// - Auth failed and host token is already expired → error with remediation hint.
pub fn run_iteration(
    cfg: &ExecutionConfig,
    iteration: u32,
    active_container: &Arc<Mutex<Option<String>>>,
) -> Result<IterationOutcome> {
    let name = container_name_for(iteration);
    let (result, status) = run_container(cfg, &name, active_container, None)?;

    if result.auth_failed
        && should_attempt_resume(
            result.session_id.as_deref(),
            cfg.credentials_file.is_some(),
            host_token_is_expired(&cfg.claude_dir),
        )
    {
        return Ok(IterationOutcome::AuthFailedResumable {
            session_id: result
                .session_id
                .expect("session_id is Some when should_attempt_resume returns true"),
        });
    }

    if let Some(e) = post_stream_error(&result, &status, "iteration") {
        return Err(e);
    }

    match result.verdict {
        Some(verdict) => Ok(IterationOutcome::Done {
            verdict,
            session_id: result.session_id,
            model_usage: result.model_usage,
            last_usage_snapshot: result.last_usage_snapshot,
        }),
        None => Ok(IterationOutcome::Continue {
            session_id: result.session_id,
            model_usage: result.model_usage,
            last_usage_snapshot: result.last_usage_snapshot,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success_status() -> std::process::ExitStatus {
        std::process::Command::new("true")
            .status()
            .expect("true command")
    }

    fn failure_status() -> std::process::ExitStatus {
        std::process::Command::new("false")
            .status()
            .expect("false command")
    }

    fn clear_result() -> StreamResult {
        StreamResult {
            auth_failed: false,
            submit_verdict_missing: false,
            verdict: None,
            session_id: None,
            model_usage: None,
            last_usage_snapshot: None,
        }
    }

    #[test]
    fn resume_attempted_when_session_id_valid_token_and_credentials() {
        assert!(should_attempt_resume(Some("sess_01"), true, false));
    }

    #[test]
    fn resume_not_attempted_when_host_token_expired() {
        assert!(!should_attempt_resume(Some("sess_01"), true, true));
    }

    #[test]
    fn resume_not_attempted_when_no_session_id() {
        assert!(!should_attempt_resume(None, true, false));
    }

    #[test]
    fn resume_not_attempted_when_no_credentials_file() {
        assert!(!should_attempt_resume(Some("sess_01"), false, false));
    }

    #[test]
    fn post_stream_error_returns_none_when_all_clear() {
        let result = clear_result();
        assert!(post_stream_error(&result, &success_status(), "test").is_none());
    }

    #[test]
    fn post_stream_error_auth_failed_returns_error() {
        let result = StreamResult {
            auth_failed: true,
            ..clear_result()
        };
        let err = post_stream_error(&result, &success_status(), "iteration").unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("authentication failed"),
            "expected auth error message: {msg}"
        );
        assert!(
            msg.contains("claude auth login"),
            "expected remediation hint: {msg}"
        );
    }

    #[test]
    fn post_stream_error_submit_verdict_missing_returns_error() {
        let result = StreamResult {
            submit_verdict_missing: true,
            ..clear_result()
        };
        let err = post_stream_error(&result, &success_status(), "iteration").unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("submit_verdict"),
            "expected tool name in error: {msg}"
        );
        assert!(
            msg.contains("MCP tool was not registered"),
            "expected registration error: {msg}"
        );
    }

    #[test]
    fn post_stream_error_non_zero_exit_returns_error() {
        let result = clear_result();
        let err = post_stream_error(&result, &failure_status(), "pipeline").unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("pipeline"),
            "expected context name in error: {msg}"
        );
        assert!(
            msg.contains("exited with code"),
            "expected exit code in error: {msg}"
        );
    }

    #[test]
    fn post_stream_error_auth_failed_takes_priority_over_non_zero_exit() {
        let result = StreamResult {
            auth_failed: true,
            ..clear_result()
        };
        let err = post_stream_error(&result, &failure_status(), "test").unwrap();
        assert!(
            err.to_string().contains("authentication failed"),
            "auth failure must take priority"
        );
    }

    #[test]
    fn post_stream_error_submit_verdict_missing_takes_priority_over_non_zero_exit() {
        let result = StreamResult {
            submit_verdict_missing: true,
            ..clear_result()
        };
        let err = post_stream_error(&result, &failure_status(), "test").unwrap();
        assert!(
            err.to_string().contains("submit_verdict"),
            "submit_verdict_missing must take priority over non-zero exit"
        );
    }

    #[test]
    fn format_tool_args_priority_key_command() {
        let input = serde_json::json!({"command": "ls -la", "other": "ignored"});
        assert_eq!(format_tool_args(&input), "ls -la");
    }

    #[test]
    fn format_tool_args_priority_key_file_path() {
        let input = serde_json::json!({"file_path": "/src/main.rs"});
        assert_eq!(format_tool_args(&input), "/src/main.rs");
    }

    #[test]
    fn format_tool_args_priority_key_path() {
        let input = serde_json::json!({"path": "/some/dir"});
        assert_eq!(format_tool_args(&input), "/some/dir");
    }

    #[test]
    fn format_tool_args_priority_key_pattern() {
        let input = serde_json::json!({"pattern": "*.rs"});
        assert_eq!(format_tool_args(&input), "*.rs");
    }

    #[test]
    fn format_tool_args_priority_key_prompt() {
        let input = serde_json::json!({"prompt": "hello world"});
        assert_eq!(format_tool_args(&input), "hello world");
    }

    #[test]
    fn format_tool_args_priority_key_over_fallback() {
        let input = serde_json::json!({"other": "fallback_value", "command": "priority_value"});
        assert_eq!(format_tool_args(&input), "priority_value");
    }

    #[test]
    fn format_tool_args_fallback_to_first_string_value() {
        let input = serde_json::json!({"unknown_key": "fallback_value"});
        assert_eq!(format_tool_args(&input), "fallback_value");
    }

    #[test]
    fn format_tool_args_non_object_returns_empty() {
        assert_eq!(format_tool_args(&serde_json::json!("string")), "");
        assert_eq!(format_tool_args(&serde_json::json!(42)), "");
        assert_eq!(format_tool_args(&serde_json::json!(["a", "b"])), "");
        assert_eq!(format_tool_args(&serde_json::json!(null)), "");
    }

    #[test]
    fn format_tool_args_empty_object_returns_empty() {
        assert_eq!(format_tool_args(&serde_json::json!({})), "");
    }

    #[test]
    fn format_tool_args_priority_key_non_string_value_falls_through_to_fallback() {
        let input = serde_json::json!({"command": 42, "other": "fallback"});
        assert_eq!(format_tool_args(&input), "fallback");
    }

    #[test]
    fn format_tool_args_newlines_replaced_with_spaces() {
        let input = serde_json::json!({"command": "line1\nline2\nline3"});
        assert_eq!(format_tool_args(&input), "line1 line2 line3");
    }

    #[test]
    fn format_tool_args_no_string_values_returns_empty() {
        let input = serde_json::json!({"a": 1, "b": true, "c": null});
        assert_eq!(format_tool_args(&input), "");
    }
}
