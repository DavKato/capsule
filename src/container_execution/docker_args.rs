use anyhow::Context as _;

use super::ExecutionConfig;

/// Returns a unique container name for the given iteration.
///
/// Format: `capsule-run-<pid>-<iteration>`.  Unique per process per iteration
/// so the ctrlc handler can call `docker stop <name>` when the user interrupts.
pub fn container_name_for(iteration: u32) -> String {
    format!("capsule-run-{}-{}", std::process::id(), iteration)
}

/// Build the `docker run` argument list for one iteration.
///
/// Extracted for testability. Adds a read-only bind-mount of `.git/config` when
/// present in `cfg.pwd`, preventing container processes from mutating the host
/// repository's remote URLs or other local git config.
pub fn build_docker_args(
    cfg: &ExecutionConfig,
    prompt_path: &std::path::Path,
    container_name: &str,
) -> anyhow::Result<Vec<String>> {
    let workspace = cfg.pwd.to_string_lossy();
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        container_name.to_string(),
        format!("-v={}:/home/claude/prompt.txt", prompt_path.display()),
        format!("-v={workspace}:{workspace}"),
        format!("--workdir={workspace}"),
        format!("-e=CAPSULE_WORKSPACE={workspace}"),
        format!("-v={}:/home/claude/.claude", cfg.claude_dir.display()),
    ];

    // Shadow the credentials file inside the directory mount with an isolated
    // per-run copy so the host and container never race over token rotation.
    if let Some(creds) = &cfg.credentials_file {
        args.push(format!(
            "-v={}:/home/claude/.claude/.credentials.json",
            creds.display()
        ));
    }

    // Protect the host git config from container mutations (issue #20).
    // If the workspace is a git repo, mount .git/config read-only so that
    // container processes (including Claude) cannot rewrite remote URLs or
    // other local settings back to the host.
    let git_config = cfg.pwd.join(".git").join("config");
    if git_config.exists() {
        args.push(format!(
            "-v={}:{workspace}/.git/config:ro",
            git_config.display()
        ));
    }

    if let Some(env_file) = &cfg.env_file {
        args.push(format!("--env-file={}", env_file.display()));
    }

    if let Some(extra_file) = &cfg.extra_env_file {
        args.push(format!("--env-file={}", extra_file.display()));
    }

    if let Some(token_file) = &cfg.gh_token_env_file {
        args.push(format!("--env-file={}", token_file.display()));
    }

    if let Some(model) = &cfg.model {
        args.push(format!("-e=CAPSULE_MODEL={model}"));
    }

    // Pass git identity to the container entrypoint so it can configure
    // `git config --global user.name/email`. The entrypoint falls back to
    // `Capsule <capsule@localhost>` when these are empty.
    args.push(format!("-e=GIT_AUTHOR_NAME={}", cfg.git_author_name));
    args.push(format!("-e=GIT_AUTHOR_EMAIL={}", cfg.git_author_email));
    args.push(format!("-e=GIT_COMMITTER_NAME={}", cfg.git_author_name));
    args.push(format!("-e=GIT_COMMITTER_EMAIL={}", cfg.git_author_email));

    if let Some(value) = &cfg.setup {
        if value.contains(char::is_whitespace) {
            args.push(format!("-e=CAPSULE_STAGE_SETUP={value}"));
        } else {
            // No whitespace → must be a file path. Error clearly if the file is missing
            // so the user gets a useful message instead of a confusing shell error from
            // `bash -c "<nonexistent>"` inside the container.
            let candidate = cfg.capsule_dir.join(value);
            candidate.exists().then_some(()).with_context(|| {
                format!(
                    "setup file not found: {value} (resolved to {}). \
                     To use an inline command, include a space (e.g. \"bash {value}\").",
                    candidate.display()
                )
            })?;
            args.push(format!(
                "-v={}:/home/claude/stage-setup.sh:ro",
                candidate.display()
            ));
            args.push("-e=CAPSULE_STAGE_SETUP=/home/claude/stage-setup.sh".to_string());
        }
    }

    if let Some(network) = &cfg.compose_network {
        args.push("--network".to_string());
        args.push(network.clone());
    }

    args.push(cfg.image.clone());
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_name_for_has_expected_format() {
        let name = container_name_for(3);
        assert!(
            name.starts_with("capsule-run-"),
            "name should start with capsule-run-: {name}"
        );
        assert!(
            name.ends_with("-3"),
            "name should end with iteration number: {name}"
        );
    }

    #[test]
    fn prompt_mount_is_not_read_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let prompt_arg = args.iter().find(|a| a.contains("prompt.txt")).unwrap();
        assert!(
            !prompt_arg.ends_with(":ro"),
            "prompt.txt must not be mounted read-only so setup can mutate it: {prompt_arg}"
        );
    }

    #[test]
    fn workspace_mounted_at_host_path_not_slash_workspace() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        let pwd_str = dir.path().to_string_lossy();
        assert!(
            joined.contains(&format!("-v={pwd_str}:{pwd_str}")),
            "workspace must be mounted at host path, not /workspace: {joined}"
        );
        assert!(
            !joined.contains(":/workspace"),
            "must not mount workspace at /workspace: {joined}"
        );
    }

    #[test]
    fn workdir_set_to_host_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        let pwd_str = dir.path().to_string_lossy();
        assert!(
            joined.contains(&format!("--workdir={pwd_str}")),
            "expected --workdir set to host path in args: {joined}"
        );
    }

    #[test]
    fn capsule_workspace_env_var_set_to_host_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        let pwd_str = dir.path().to_string_lossy();
        assert!(
            joined.contains(&format!("-e=CAPSULE_WORKSPACE={pwd_str}")),
            "expected -e=CAPSULE_WORKSPACE=<host-path> in args: {joined}"
        );
    }

    #[test]
    fn env_file_arg_present_when_file_exists() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join(".env"), "FOO=bar\n").unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            env_file: Some(dir.path().join(".env")),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("--env-file"),
            "expected --env-file in args: {joined}"
        );
        assert!(
            joined.contains(".env"),
            "expected .env path in args: {joined}"
        );
    }

    #[test]
    fn env_file_arg_absent_when_no_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            !joined.contains("--env-file"),
            "expected no --env-file when env_file is None: {joined}"
        );
    }

    #[test]
    fn gh_token_env_file_passed_when_present() {
        let dir = tempfile::tempdir().expect("temp dir");
        let token_file = dir.path().join("gh-token.env");
        std::fs::write(&token_file, "GH_TOKEN=ghs_testtoken\n").unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            gh_token_env_file: Some(token_file.clone()),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("--env-file"),
            "expected --env-file for gh token: {joined}"
        );
        assert!(
            joined.contains("gh-token.env"),
            "expected token file path in args: {joined}"
        );
    }

    #[test]
    fn extra_env_file_emitted_after_primary_env_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let primary = dir.path().join(".env");
        let extra = dir.path().join("extra.env");
        std::fs::write(&primary, "FOO=default\n").unwrap();
        std::fs::write(&extra, "FOO=override\n").unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            env_file: Some(primary.clone()),
            extra_env_file: Some(extra.clone()),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let primary_pos = args
            .iter()
            .position(|a| a.contains(".env") && !a.contains("extra"))
            .expect("primary env-file not found");
        let extra_pos = args
            .iter()
            .position(|a| a.contains("extra.env"))
            .expect("extra env-file not found");
        assert!(
            primary_pos < extra_pos,
            "primary .env must appear before extra.env (override ordering)"
        );
    }

    #[test]
    fn extra_env_file_absent_when_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            !joined.contains("extra"),
            "extra env-file must not appear when None: {joined}"
        );
    }

    #[test]
    fn gh_token_not_in_docker_args_when_env_file_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            !joined.contains("GH_TOKEN"),
            "token must not appear in docker args: {joined}"
        );
    }

    #[test]
    fn gh_token_never_appears_inline_in_docker_args() {
        let dir = tempfile::tempdir().expect("temp dir");
        let token_file = dir.path().join("gh-token.env");
        std::fs::write(&token_file, "GH_TOKEN=ghs_secret\n").unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            gh_token_env_file: Some(token_file),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        for arg in &args {
            assert!(
                !arg.contains("ghs_secret"),
                "token value must not appear inline: {arg}"
            );
        }
    }

    #[test]
    fn git_config_mounted_readonly_when_present() {
        let dir = tempfile::tempdir().expect("temp dir");
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(
            git_dir.join("config"),
            "[core]\n\trepositoryformatversion = 0\n",
        )
        .unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        let pwd_str = dir.path().to_string_lossy();
        assert!(
            joined.contains(&format!(".git/config:{pwd_str}/.git/config:ro")),
            "expected read-only git config mount at host path in args: {joined}"
        );
    }

    #[test]
    fn git_config_mount_absent_when_no_git_dir() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            !joined.contains(".git/config"),
            "expected no git config mount when .git/config absent: {joined}"
        );
    }

    #[test]
    fn git_identity_env_vars_present_in_docker_args() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            git_author_name: "Bob Builder".to_string(),
            git_author_email: "bob@example.com".to_string(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("GIT_AUTHOR_NAME=Bob Builder"),
            "expected GIT_AUTHOR_NAME: {joined}"
        );
        assert!(
            joined.contains("GIT_AUTHOR_EMAIL=bob@example.com"),
            "expected GIT_AUTHOR_EMAIL: {joined}"
        );
        assert!(
            joined.contains("GIT_COMMITTER_NAME=Bob Builder"),
            "expected GIT_COMMITTER_NAME: {joined}"
        );
        assert!(
            joined.contains("GIT_COMMITTER_EMAIL=bob@example.com"),
            "expected GIT_COMMITTER_EMAIL: {joined}"
        );
    }

    #[test]
    fn git_identity_env_vars_present_when_empty() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("GIT_AUTHOR_NAME="),
            "expected GIT_AUTHOR_NAME= in args: {joined}"
        );
        assert!(
            joined.contains("GIT_AUTHOR_EMAIL="),
            "expected GIT_AUTHOR_EMAIL= in args: {joined}"
        );
    }

    #[test]
    fn setup_file_path_mounted_and_env_var_set() {
        let capsule_dir = tempfile::tempdir().expect("capsule temp dir");
        let script = capsule_dir.path().join("setup.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        let pwd = tempfile::tempdir().expect("pwd temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: pwd.path().to_path_buf(),
            capsule_dir: capsule_dir.path().to_path_buf(),
            setup: Some("setup.sh".to_string()),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("/home/claude/stage-setup.sh:ro"),
            "expected stage-setup.sh read-only mount: {joined}"
        );
        assert!(
            joined.contains(capsule_dir.path().to_string_lossy().as_ref()),
            "expected host script path in mount: {joined}"
        );
        assert!(
            joined.contains("CAPSULE_STAGE_SETUP=/home/claude/stage-setup.sh"),
            "expected CAPSULE_STAGE_SETUP set to container path: {joined}"
        );
    }

    #[test]
    fn setup_inline_command_sets_env_var_without_mount() {
        let capsule_dir = tempfile::tempdir().expect("capsule temp dir");
        let pwd = tempfile::tempdir().expect("pwd temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: pwd.path().to_path_buf(),
            capsule_dir: capsule_dir.path().to_path_buf(),
            setup: Some("pip install -r requirements.txt".to_string()),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("CAPSULE_STAGE_SETUP=pip install -r requirements.txt"),
            "expected CAPSULE_STAGE_SETUP set to inline command: {joined}"
        );
        assert!(
            !joined.contains("stage-setup.sh"),
            "inline command must not produce a file mount: {joined}"
        );
    }

    #[test]
    fn setup_missing_file_returns_clear_error() {
        let capsule_dir = tempfile::tempdir().expect("capsule temp dir");
        let pwd = tempfile::tempdir().expect("pwd temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: pwd.path().to_path_buf(),
            capsule_dir: capsule_dir.path().to_path_buf(),
            setup: Some("nonexistent.sh".to_string()),
            ..ExecutionConfig::default()
        };
        let err = build_docker_args(&cfg, prompt_file.path(), "capsule-test")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("setup file not found: nonexistent.sh"),
            "expected clear error message, got: {err}"
        );
    }

    #[test]
    fn setup_absent_means_no_capsule_stage_setup_env_var() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            !joined.contains("CAPSULE_STAGE_SETUP"),
            "CAPSULE_STAGE_SETUP must not appear when setup is None: {joined}"
        );
        assert!(
            !joined.contains("stage-setup.sh"),
            "stage-setup.sh must not appear when setup is None: {joined}"
        );
    }

    #[test]
    fn model_arg_present_when_model_set() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            model: Some("claude-opus-4-6".to_string()),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("-e=CAPSULE_MODEL=claude-opus-4-6"),
            "expected -e=CAPSULE_MODEL=claude-opus-4-6 in args: {joined}"
        );
    }

    #[test]
    fn model_arg_absent_when_no_model() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            !joined.contains("CAPSULE_MODEL"),
            "CAPSULE_MODEL must not appear in args when model is None: {joined}"
        );
    }

    #[test]
    fn verbose_flag_not_added_to_docker_args() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg_verbose = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            verbose: true,
            ..ExecutionConfig::default()
        };
        let cfg_quiet = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args_verbose =
            build_docker_args(&cfg_verbose, prompt_file.path(), "capsule-test").unwrap();
        let args_quiet = build_docker_args(&cfg_quiet, prompt_file.path(), "capsule-test").unwrap();
        assert_eq!(
            args_verbose, args_quiet,
            "verbose flag must not alter docker args"
        );
    }

    #[test]
    fn container_name_present_in_docker_args() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-run-12345-1").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("--name capsule-run-12345-1"),
            "expected --name in args: {joined}"
        );
    }

    #[test]
    fn compose_network_arg_present_when_set() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            compose_network: Some("myproject_default".to_string()),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("--network myproject_default"),
            "expected --network in args: {joined}"
        );
    }

    #[test]
    fn compose_network_arg_absent_when_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            !joined.contains("--network"),
            "expected no --network when compose_network is None: {joined}"
        );
    }

    #[test]
    fn credentials_file_shadowed_over_claude_dir_mount() {
        let dir = tempfile::tempdir().expect("temp dir");
        let creds_file = tempfile::NamedTempFile::new().unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            credentials_file: Some(creds_file.path().to_path_buf()),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains(":/home/claude/.claude/.credentials.json"),
            "expected credentials shadow mount in args: {joined}"
        );
        assert!(
            joined.contains(creds_file.path().to_string_lossy().as_ref()),
            "expected temp credentials path in mount: {joined}"
        );
    }

    #[test]
    fn credentials_file_absent_when_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            credentials_file: None,
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            !joined.contains(".credentials.json"),
            "expected no credentials mount when credentials_file is None: {joined}"
        );
    }

    #[test]
    fn claude_dir_mounted_at_home_claude_dot_claude() {
        let dir = tempfile::tempdir().expect("temp dir");
        let claude_dir = tempfile::tempdir().expect("claude temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = ExecutionConfig {
            pwd: dir.path().to_path_buf(),
            claude_dir: claude_dir.path().to_path_buf(),
            ..ExecutionConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test").unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains(":/home/claude/.claude"),
            "expected ~/.claude mount in args: {joined}"
        );
        assert!(
            joined.contains(claude_dir.path().to_string_lossy().as_ref()),
            "expected host claude_dir path in mount: {joined}"
        );
    }
}
