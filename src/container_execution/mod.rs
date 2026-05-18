mod docker_args;
mod infra;
mod process;
mod runner;
mod stream_parser;

pub use docker_args::{build_docker_args, container_name_for};
pub use infra::{
    detect_compose_network, host_token_is_expired, make_mcp_config, token_remaining_minutes,
};
pub use process::{post_stream_error, run_container, run_iteration, StreamResult};
pub use runner::{CredentialsGuard, DockerStageRunner};
pub use stream_parser::{
    format_usage_with_percentage, ModelUsage, StreamParser, ToolEvent, ToolResultEvent,
    ToolUseEvent, UsageSnapshot,
};

use std::path::PathBuf;

/// Configuration for a single iteration's `docker run`.
#[derive(Default, Clone)]
pub struct ExecutionConfig {
    /// Docker image to run (base or derived).
    pub image: String,
    /// Prompt content to mount as `/home/claude/prompt.txt`.
    pub prompt: String,
    /// Host working directory — mounted as `/workspace`.
    pub pwd: PathBuf,
    /// Optional model override passed via `-e CAPSULE_MODEL`.
    pub model: Option<String>,
    /// When true, print unfiltered container output to stderr in addition to the display view.
    pub verbose: bool,
    /// Path to the `.env` file to pass via `--env-file` (None → omitted).
    pub env_file: Option<PathBuf>,
    /// Path to a temp env-file written from `--env KEY=VALUE` pairs.
    /// Emitted after `env_file` so user overrides take precedence over `.capsule/.env`.
    pub extra_env_file: Option<PathBuf>,
    /// Path to a temp env-file containing `GH_TOKEN=<token>` (None → no token injected).
    /// Passed as a final `--env-file` so the token never appears in the process arg list.
    pub gh_token_env_file: Option<PathBuf>,
    /// Git author/committer name passed as `GIT_AUTHOR_NAME` and `GIT_COMMITTER_NAME`.
    pub git_author_name: String,
    /// Git author/committer email passed as `GIT_AUTHOR_EMAIL` and `GIT_COMMITTER_EMAIL`.
    pub git_author_email: String,
    /// Per-stage setup value from `StageConfig.setup`. If the value resolves to an
    /// existing file under `capsule_dir`, the file is bind-mounted read-only and
    /// `CAPSULE_STAGE_SETUP` is set to the container-side path; otherwise the value
    /// is treated as an inline shell command and passed directly as the env var.
    pub setup: Option<String>,
    /// `.capsule/` directory on the host — used to resolve relative `setup` file paths.
    pub capsule_dir: PathBuf,
    /// Docker network to attach the container to. Detected from a running Compose
    /// project at `pwd`; None when no project is found.
    pub compose_network: Option<String>,
    /// Host `~/.claude` directory, mounted writable at `/home/claude/.claude` so
    /// the container can authenticate and share memory/sessions with the host.
    pub claude_dir: PathBuf,
    /// Isolated copy of `~/.claude/.credentials.json`, mounted over the directory
    /// mount to prevent concurrent host/container token rotation from invalidating
    /// each other's sessions (issue #55). `None` when the credentials file does not
    /// exist on the host.
    pub credentials_file: Option<PathBuf>,
}

/// Outcome of a single iteration.
#[derive(Debug)]
pub enum IterationOutcome {
    /// Loop should continue to the next iteration.
    Continue {
        session_id: Option<String>,
        model_usage: Option<ModelUsage>,
        last_usage_snapshot: Option<UsageSnapshot>,
    },
    /// Claude submitted a verdict; loop should stop.
    Done {
        verdict: crate::verdict::Verdict,
        session_id: Option<String>,
        model_usage: Option<ModelUsage>,
        last_usage_snapshot: Option<UsageSnapshot>,
    },
    /// Auth failed but the host token is still valid; caller should re-copy
    /// credentials, call `CredentialsGuard::reset_baseline`, and retry with
    /// `run_container(..., Some(session_id))`.
    AuthFailedResumable { session_id: String },
}
