use anyhow::{Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::display::RetryInfo;
use crate::pipeline::StageRunner;
use crate::verdict::Verdict;

use super::{
    container_name_for, post_stream_error, run_container, run_iteration, ExecutionConfig,
    IterationOutcome,
};

pub struct CredentialsGuard {
    tempfile: tempfile::NamedTempFile,
    original_bytes: Vec<u8>,
    host_mtime: SystemTime,
    claude_dir: PathBuf,
}

impl CredentialsGuard {
    pub fn new(claude_dir: &std::path::Path) -> Result<Option<Self>> {
        let src = claude_dir.join(".credentials.json");
        if !src.exists() {
            return Ok(None);
        }
        let host_mtime = src
            .metadata()
            .and_then(|m| m.modified())
            .context("failed to read credentials file mtime")?;
        let content =
            std::fs::read(&src).with_context(|| format!("failed to read {}", src.display()))?;
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
            host_mtime,
            claude_dir: claude_dir.to_path_buf(),
        }))
    }

    pub fn path(&self) -> &std::path::Path {
        self.tempfile.path()
    }

    /// Re-read host mtime and temp file content after an external mutation of the
    /// temp file (e.g. re-copying host credentials for a resume-retry). Resets the
    /// write-back baseline so the guard correctly detects further token rotations.
    fn reset_baseline(&mut self) -> Result<()> {
        let src = self.claude_dir.join(".credentials.json");
        self.host_mtime = src
            .metadata()
            .and_then(|m| m.modified())
            .context("failed to re-read credentials mtime")?;
        self.original_bytes = std::fs::read(self.tempfile.path())
            .context("failed to re-read credentials temp file")?;
        Ok(())
    }
}

impl Drop for CredentialsGuard {
    fn drop(&mut self) {
        let dest = self.claude_dir.join(".credentials.json");
        // Skip write-back if the host refreshed its token during the run.
        if let Ok(current_mtime) = dest.metadata().and_then(|m| m.modified()) {
            if current_mtime != self.host_mtime {
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
        if let Err(e) = std::fs::copy(self.tempfile.path(), &dest) {
            crate::display::warning(&format!("failed to write back credentials: {e}"));
        }
    }
}

pub struct DockerStageRunner {
    base_cfg: ExecutionConfig,
    active_container: Arc<Mutex<Option<String>>>,
    iteration: u32,
    session_id: Option<String>,
    credentials_guard: Option<CredentialsGuard>,
    resume_session_id: Option<String>,
}

impl DockerStageRunner {
    pub fn new(
        base_cfg: ExecutionConfig,
        active_container: Arc<Mutex<Option<String>>>,
        credentials_guard: Option<CredentialsGuard>,
        resume_session_id: Option<String>,
    ) -> Self {
        Self {
            base_cfg,
            active_container,
            iteration: 0,
            session_id: None,
            credentials_guard,
            resume_session_id,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

impl StageRunner for DockerStageRunner {
    fn run(
        &mut self,
        stage_name: &str,
        prompt: &str,
        model: Option<&str>,
        retry: Option<&RetryInfo>,
    ) -> anyhow::Result<Option<Verdict>> {
        self.iteration += 1;
        let effective_model = model
            .or(self.base_cfg.model.as_deref())
            .unwrap_or("unknown");
        crate::display::stage_header(stage_name, self.iteration, effective_model, retry);
        let start = std::time::Instant::now();
        let result = self.execute_stage(prompt, model);
        let duration = start.elapsed();
        if let Ok(ref verdict) = result {
            crate::display::session_footer(verdict.as_ref(), duration, self.session_id.as_deref());
        }
        result
    }
}

impl DockerStageRunner {
    fn execute_stage(
        &mut self,
        prompt: &str,
        model: Option<&str>,
    ) -> anyhow::Result<Option<Verdict>> {
        let mut cfg = self.base_cfg.clone();
        cfg.prompt = prompt.to_string();
        if let Some(m) = model {
            cfg.model = Some(m.to_string());
        }
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
            return Ok(result.verdict);
        }
        match run_iteration(&cfg, self.iteration, &self.active_container)? {
            IterationOutcome::Done {
                verdict,
                session_id,
            } => {
                if let Some(id) = session_id {
                    self.session_id = Some(id);
                }
                Ok(Some(verdict))
            }
            IterationOutcome::Continue { session_id } => {
                if let Some(id) = session_id {
                    self.session_id = Some(id);
                }
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
            "[capsule] auth failed — host token valid, attempting resume-retry (session {session_id})"
        ));
        if let Some(ref mut guard) = self.credentials_guard {
            let host_creds = cfg.claude_dir.join(".credentials.json");
            std::fs::copy(&host_creds, guard.path())
                .context("failed to re-copy credentials for resume-retry")?;
            guard.reset_baseline()?;
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
        Ok(result.verdict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_written_back_after_reset_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, b"original").unwrap();

        let mut guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        // Simulate re-copying host creds to the temp file (as resume-retry does).
        std::fs::write(guard.path(), b"resumed-creds").unwrap();
        guard.reset_baseline().unwrap();
        // Simulate the resumed container rotating the token.
        std::fs::write(guard.path(), b"rotated-by-resume").unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), b"rotated-by-resume");
    }

    #[test]
    fn credentials_written_back_when_container_refreshed_and_host_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, b"original").unwrap();

        let guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        std::fs::write(guard.path(), b"refreshed").unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), b"refreshed");
    }

    #[test]
    fn credentials_not_written_back_when_host_modified_during_run() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, b"original").unwrap();

        let guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        std::fs::write(guard.path(), b"container-refreshed").unwrap();
        std::fs::write(&creds_path, b"host-refreshed").unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), b"host-refreshed");
    }

    #[test]
    fn credentials_unchanged_when_container_did_not_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, b"original").unwrap();

        let guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), b"original");
    }
}
