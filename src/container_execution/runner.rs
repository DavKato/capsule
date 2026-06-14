use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::pipeline::{RetryInfo, StageRunner};
use crate::verdict::Verdict;

use super::credentials_source::{CredentialsSource, HostRevision};
use super::{
    container_name_for, post_stream_error, run_container, run_iteration, ExecutionConfig,
    IterationOutcome, ModelUsage, UsageSnapshot,
};

pub struct CredentialsGuard {
    tempfile: tempfile::NamedTempFile,
    original_bytes: Vec<u8>,
    host_revision: HostRevision,
    source: CredentialsSource,
}

impl CredentialsGuard {
    pub fn new(claude_dir: &std::path::Path) -> Result<Option<Self>> {
        let source = CredentialsSource::detect(claude_dir);
        let content = match source.read()? {
            Some(c) => c,
            None => return Ok(None),
        };
        let host_revision = source.revision()?;
        let mut tmp = tempfile::Builder::new()
            .prefix("capsule-credentials-")
            .suffix(".json")
            .tempfile()
            .context("failed to create credentials temp file")?;
        tmp.write_all(&content)
            .context("failed to write credentials temp file")?;
        Ok(Some(Self {
            tempfile: tmp,
            original_bytes: content,
            host_revision,
            source,
        }))
    }

    pub fn path(&self) -> &std::path::Path {
        self.tempfile.path()
    }

    /// Reload the host's current credentials through the source into the temp file
    /// and reset the write-back baseline. Used for resume-retry, where the host may
    /// have rotated its token; works identically for file- and Keychain-backed
    /// sources. Resets the baseline so the guard correctly detects further token
    /// rotations after the reload.
    fn reload_from_host(&mut self) -> Result<()> {
        let content = self
            .source
            .read()?
            .context("host credentials disappeared during resume-retry")?;
        std::fs::write(self.tempfile.path(), &content)
            .context("failed to write reloaded credentials to temp file")?;
        self.host_revision = self.source.revision()?;
        self.original_bytes = content;
        Ok(())
    }
}

impl Drop for CredentialsGuard {
    fn drop(&mut self) {
        // Skip write-back if the host rotated its token during the run (host
        // revision changed since the snapshot). If the revision can't be read,
        // proceed — best-effort, matching the file-mtime behavior.
        if let Ok(current) = self.source.revision() {
            if current != self.host_revision {
                return;
            }
        }
        // Skip write-back if the container never refreshed.
        let current = match std::fs::read(self.tempfile.path()) {
            Ok(b) => b,
            Err(e) => {
                crate::display::warning(&format!("failed to read credentials temp file: {e}"));
                return;
            }
        };
        if current == self.original_bytes {
            return;
        }
        if let Err(e) = self.source.write(&current) {
            crate::display::warning(&format!("failed to write back credentials: {e}"));
        }
    }
}

pub struct DockerStageRunner {
    base_cfg: ExecutionConfig,
    active_container: Arc<Mutex<Option<String>>>,
    iteration: u32,
    session_id: Option<String>,
    model_usage: Option<ModelUsage>,
    last_usage_snapshot: Option<UsageSnapshot>,
    credentials_guard: Option<CredentialsGuard>,
    resume_session_id: Option<String>,
    /// Merged volumes (top-level + per-stage) keyed by stage name.
    /// Falls back to `base_cfg.volumes` for unknown stage names.
    stage_volumes: HashMap<String, Vec<String>>,
}

impl DockerStageRunner {
    pub fn new(
        base_cfg: ExecutionConfig,
        active_container: Arc<Mutex<Option<String>>>,
        credentials_guard: Option<CredentialsGuard>,
        resume_session_id: Option<String>,
        stage_volumes: HashMap<String, Vec<String>>,
    ) -> Self {
        Self {
            base_cfg,
            active_container,
            iteration: 0,
            session_id: None,
            model_usage: None,
            last_usage_snapshot: None,
            credentials_guard,
            resume_session_id,
            stage_volumes,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn model_usage(&self) -> Option<&ModelUsage> {
        self.model_usage.as_ref()
    }
}

impl StageRunner for DockerStageRunner {
    fn run(
        &mut self,
        stage_name: &str,
        prompt: &str,
        model: Option<&str>,
        setup: Option<&str>,
        retry: Option<&RetryInfo>,
    ) -> anyhow::Result<Option<Verdict>> {
        self.iteration += 1;
        let effective_model = model
            .or(self.base_cfg.model.as_deref())
            .unwrap_or("unknown");
        crate::display::stage_header(stage_name, self.iteration, effective_model, retry);
        let start = std::time::Instant::now();
        let volumes = self
            .stage_volumes
            .get(stage_name)
            .cloned()
            .unwrap_or_else(|| self.base_cfg.volumes.clone());
        let result = self.execute_stage(prompt, model, setup, volumes);
        let duration = start.elapsed();
        crate::display::clear_stage();
        let usage_str = self
            .last_usage_snapshot
            .as_ref()
            .zip(self.model_usage.as_ref())
            .map(|(snap, u)| {
                super::format_usage_with_percentage(snap.total_tokens(), u.context_window)
            });
        match &result {
            Ok(verdict) => {
                crate::display::session_footer(
                    stage_name,
                    self.iteration,
                    verdict.as_ref(),
                    duration,
                    self.session_id.as_deref(),
                    usage_str.as_deref(),
                );
            }
            Err(e) => {
                let error_verdict = crate::verdict::Verdict {
                    status: crate::verdict::VerdictStatus::Fail,
                    notes: Some(format!("{e:#}")),
                };
                crate::display::session_footer(
                    stage_name,
                    self.iteration,
                    Some(&error_verdict),
                    duration,
                    self.session_id.as_deref(),
                    usage_str.as_deref(),
                );
            }
        }
        result
    }
}

impl DockerStageRunner {
    fn execute_stage(
        &mut self,
        prompt: &str,
        model: Option<&str>,
        setup: Option<&str>,
        volumes: Vec<String>,
    ) -> anyhow::Result<Option<Verdict>> {
        let mut cfg = self.base_cfg.clone();
        cfg.prompt = prompt.to_string();
        if let Some(m) = model {
            cfg.model = Some(m.to_string());
        }
        cfg.setup = setup.map(|s| s.to_string());
        cfg.volumes = volumes;
        if let Some(session_id) = self.resume_session_id.take() {
            let name = format!("{}-resume-pipeline", container_name_for(self.iteration));
            let (result, status) =
                run_container(&cfg, &name, &self.active_container, Some(&session_id))?;
            if let Some(e) = post_stream_error(&result, &status, "pipeline-resume") {
                return Err(e);
            }
            if let Some(id) = result.session_id {
                self.session_id = Some(id);
            }
            if let Some(u) = result.model_usage {
                self.model_usage = Some(u);
            }
            self.last_usage_snapshot = result.last_usage_snapshot;
            return Ok(result.verdict);
        }
        match run_iteration(&cfg, self.iteration, &self.active_container)? {
            IterationOutcome::Done {
                verdict,
                session_id,
                model_usage,
                last_usage_snapshot,
            } => {
                if let Some(id) = session_id {
                    self.session_id = Some(id);
                }
                if let Some(u) = model_usage {
                    self.model_usage = Some(u);
                }
                self.last_usage_snapshot = last_usage_snapshot;
                Ok(Some(verdict))
            }
            IterationOutcome::Continue {
                session_id,
                model_usage,
                last_usage_snapshot,
            } => {
                if let Some(id) = session_id {
                    self.session_id = Some(id);
                }
                if let Some(u) = model_usage {
                    self.model_usage = Some(u);
                }
                self.last_usage_snapshot = last_usage_snapshot;
                Ok(None)
            }
            IterationOutcome::AuthFailedResumable { session_id } => {
                self.run_resume(&cfg, &session_id)
            }
        }
    }
}

impl DockerStageRunner {
    /// Re-copy host credentials, reset the guard baseline, and launch a resume container.
    /// Returns the verdict if the resumed session submits one.
    fn run_resume(
        &mut self,
        cfg: &ExecutionConfig,
        session_id: &str,
    ) -> anyhow::Result<Option<Verdict>> {
        crate::display::warning(&format!(
            "auth failed — host token valid, attempting resume-retry (session {session_id})"
        ));
        if let Some(ref mut guard) = self.credentials_guard {
            guard.reload_from_host()?;
        }
        let resume_name = format!("{}-resume", container_name_for(self.iteration));
        let (result, status) =
            run_container(cfg, &resume_name, &self.active_container, Some(session_id))?;
        if let Some(e) = post_stream_error(&result, &status, "resume-retry") {
            return Err(e);
        }
        if let Some(id) = result.session_id {
            self.session_id = Some(id);
        }
        if let Some(u) = result.model_usage {
            self.model_usage = Some(u);
        }
        self.last_usage_snapshot = result.last_usage_snapshot;
        Ok(result.verdict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A credentials blob carrying `token` as the OAuth access token. The guard
    /// only cares about byte equality and mtime, but the content must look like
    /// real credentials so `CredentialsSource::detect` selects the File source on
    /// every platform (on macOS, a non-credential file falls back to the
    /// Keychain — see `credentials_source`).
    fn creds(token: &str) -> Vec<u8> {
        format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"}}}}"#).into_bytes()
    }

    #[test]
    fn credentials_written_back_after_reload_from_host() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, creds("original")).unwrap();

        let mut guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        // Host rotated its token; resume-retry reloads it into the temp file.
        std::fs::write(&creds_path, creds("host-rotated")).unwrap();
        guard.reload_from_host().unwrap();
        assert_eq!(std::fs::read(guard.path()).unwrap(), creds("host-rotated"));
        // The resumed container then refreshes again.
        std::fs::write(guard.path(), creds("rotated-by-resume")).unwrap();
        drop(guard);

        assert_eq!(
            std::fs::read(&creds_path).unwrap(),
            creds("rotated-by-resume")
        );
    }

    #[test]
    fn credentials_written_back_when_container_refreshed_and_host_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, creds("original")).unwrap();

        let guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        std::fs::write(guard.path(), creds("refreshed")).unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), creds("refreshed"));
    }

    #[test]
    fn credentials_not_written_back_when_host_modified_during_run() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, creds("original")).unwrap();

        let guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        std::fs::write(guard.path(), creds("container-refreshed")).unwrap();
        std::fs::write(&creds_path, creds("host-refreshed")).unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), creds("host-refreshed"));
    }

    #[test]
    fn credentials_unchanged_when_container_did_not_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, creds("original")).unwrap();

        let guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), creds("original"));
    }
}
