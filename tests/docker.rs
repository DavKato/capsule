use capsule::docker::{
    build_base_image, build_docker_args, contains_auth_failure, contains_no_more_tasks,
    run_iteration, IterationOutcome, RunConfig, DOCKERFILE, STREAM_DISPLAY_JQ,
};

// ── Unit tests ────────────────────────────────────────────────────────────────

#[test]
fn embedded_dockerfile_is_non_empty() {
    assert!(
        !DOCKERFILE.is_empty(),
        "embedded Dockerfile must not be empty"
    );
    assert!(
        DOCKERFILE.contains("FROM archlinux"),
        "Dockerfile must start from archlinux base"
    );
}

#[test]
fn embedded_stream_display_jq_is_non_empty() {
    assert!(
        !STREAM_DISPLAY_JQ.is_empty(),
        "embedded stream_display.jq must not be empty"
    );
    assert!(
        STREAM_DISPLAY_JQ.contains("fromjson"),
        "jq filter must contain fromjson"
    );
}

// ── Unit tests: auth failure detection ────────────────────────────────────────

#[test]
fn auth_failure_detected_in_output() {
    let line = r#"{"type":"result","subtype":"error","error":"authentication_failed"}"#;
    assert!(contains_auth_failure(line));
}

#[test]
fn auth_failure_not_triggered_on_normal_output() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#;
    assert!(!contains_auth_failure(line));
}

#[test]
fn auth_failure_not_triggered_on_empty() {
    assert!(!contains_auth_failure(""));
}

// ── Unit tests: NO MORE TASKS detection ──────────────────────────────────────

#[test]
fn no_more_tasks_detected_in_result_line() {
    let line =
        r#"{"type":"result","subtype":"success","result":"<promise>NO MORE TASKS</promise>"}"#;
    assert!(contains_no_more_tasks(line));
}

#[test]
fn no_more_tasks_not_triggered_on_normal_output() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#;
    assert!(!contains_no_more_tasks(line));
}

#[test]
fn no_more_tasks_not_triggered_on_empty() {
    assert!(!contains_no_more_tasks(""));
}

// ── Unit tests: build_docker_args (env_file + gh_token) ──────────────────────

#[test]
fn env_file_arg_present_when_file_exists() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join(".env"), "FOO=bar\n").unwrap();

    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: Some(dir.path().join(".env")),
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
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
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    assert!(
        !joined.contains("--env-file"),
        "expected no --env-file when env_file is None: {joined}"
    );
}

#[test]
fn gh_token_passed_as_explicit_env_var() {
    let dir = tempfile::tempdir().expect("temp dir");

    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: Some("ghs_testtoken".to_string()),
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    assert!(
        joined.contains("GH_TOKEN=ghs_testtoken"),
        "expected GH_TOKEN in args: {joined}"
    );
}

#[test]
fn gh_token_absent_when_none() {
    let dir = tempfile::tempdir().expect("temp dir");

    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    assert!(
        !joined.contains("GH_TOKEN"),
        "expected no GH_TOKEN when gh_token is None: {joined}"
    );
}

// ── Unit tests: build_docker_args (git config protection) ────────────────────

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
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    assert!(
        joined.contains(".git/config:/workspace/.git/config:ro"),
        "expected read-only git config mount in args: {joined}"
    );
}

#[test]
fn git_config_mount_absent_when_no_git_dir() {
    let dir = tempfile::tempdir().expect("temp dir");

    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    assert!(
        !joined.contains(".git/config"),
        "expected no git config mount when .git/config absent: {joined}"
    );
}

// ── Unit tests: build_docker_args (git identity) ─────────────────────────────

#[test]
fn git_identity_env_vars_present_in_docker_args() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "Bob Builder".to_string(),
        git_author_email: "bob@example.com".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    assert!(
        joined.contains("GIT_AUTHOR_NAME=Bob Builder"),
        "expected GIT_AUTHOR_NAME in args: {joined}"
    );
    assert!(
        joined.contains("GIT_AUTHOR_EMAIL=bob@example.com"),
        "expected GIT_AUTHOR_EMAIL in args: {joined}"
    );
    assert!(
        joined.contains("GIT_COMMITTER_NAME=Bob Builder"),
        "expected GIT_COMMITTER_NAME in args: {joined}"
    );
    assert!(
        joined.contains("GIT_COMMITTER_EMAIL=bob@example.com"),
        "expected GIT_COMMITTER_EMAIL in args: {joined}"
    );
}

#[test]
fn git_identity_env_vars_present_when_empty() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    // Even empty values should be passed so the entrypoint can fall back.
    assert!(
        joined.contains("GIT_AUTHOR_NAME="),
        "expected GIT_AUTHOR_NAME= in args: {joined}"
    );
    assert!(
        joined.contains("GIT_AUTHOR_EMAIL="),
        "expected GIT_AUTHOR_EMAIL= in args: {joined}"
    );
}

// ── Unit tests: build_docker_args (before-each.sh) ───────────────────────────

#[test]
fn before_each_mounted_when_path_provided() {
    let dir = tempfile::tempdir().expect("temp dir");
    let before_each = dir.path().join("before-each.sh");
    std::fs::write(&before_each, "#!/bin/sh\n").unwrap();

    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: Some(before_each.clone()),
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    assert!(
        joined.contains("/home/claude/before-each.sh:ro"),
        "expected before-each.sh mount in args: {joined}"
    );
    assert!(
        joined.contains(&before_each.to_string_lossy().as_ref()),
        "expected host path in mount: {joined}"
    );
}

#[test]
fn before_each_not_mounted_when_absent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    assert!(
        !joined.contains("before-each.sh"),
        "before-each.sh must not appear in args when path is None: {joined}"
    );
}

// ── Unit tests: build_docker_args (model + verbose) ──────────────────────────

#[test]
fn model_arg_present_when_model_set() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: Some("claude-opus-4-6".to_string()),
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
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
    let cfg = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let args = build_docker_args(&cfg, prompt_file.path());
    let joined = args.join(" ");
    assert!(
        !joined.contains("CAPSULE_MODEL"),
        "CAPSULE_MODEL must not appear in args when model is None: {joined}"
    );
}

#[test]
fn verbose_flag_not_added_to_docker_args() {
    // verbose is host-side behavior; it must not add extra docker flags.
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg_verbose = RunConfig {
        image: "capsule".to_string(),
        prompt: "test".to_string(),
        pwd: dir.path().to_path_buf(),
        capsule_dir: dir.path().to_path_buf(),
        model: None,
        verbose: true,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    };
    let cfg_quiet = RunConfig {
        verbose: false,
        ..RunConfig {
            image: "capsule".to_string(),
            prompt: "test".to_string(),
            pwd: dir.path().to_path_buf(),
            capsule_dir: dir.path().to_path_buf(),
            model: None,
            verbose: false,
            env_file: None,
            gh_token: None,
            git_author_name: "".to_string(),
            git_author_email: "".to_string(),
            before_each_path: None,
        }
    };
    let args_verbose = build_docker_args(&cfg_verbose, prompt_file.path());
    let args_quiet = build_docker_args(&cfg_quiet, prompt_file.path());
    assert_eq!(
        args_verbose, args_quiet,
        "verbose flag must not alter docker args"
    );
}

// ── Integration tests (require Docker daemon) ─────────────────────────────────
// Run with: cargo test -- --ignored

/// When no `capsule` image exists, `build_base_image(false)` should build it.
/// NOTE: This test pulls/builds a real Docker image — slow on first run.
#[test]
#[ignore]
fn build_base_image_creates_image_when_absent() {
    // Remove image first so we start from a known state.
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();

    build_base_image(false).expect("build_base_image should succeed");

    let out = std::process::Command::new("docker")
        .args(["image", "inspect", "capsule"])
        .output()
        .expect("docker inspect should run");
    assert!(
        out.status.success(),
        "capsule image should exist after build"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();
}

/// When image already exists, `build_base_image(false)` should skip the build
/// (observable: function returns Ok without invoking a long build).
#[test]
#[ignore]
fn build_base_image_skips_when_image_present() {
    // Ensure image exists using a trivial image (busybox tagged as capsule).
    let _ = std::process::Command::new("docker")
        .args(["pull", "busybox:latest"])
        .output();
    std::process::Command::new("docker")
        .args(["tag", "busybox:latest", "capsule"])
        .output()
        .expect("docker tag should succeed");

    // Should succeed quickly (no build needed).
    build_base_image(false).expect("build_base_image should succeed when image present");

    // Image still present, was not rebuilt (we tagged busybox; if rebuilt it would
    // be an archlinux image — but we only check it exists and call returns Ok).
    let out = std::process::Command::new("docker")
        .args(["image", "inspect", "capsule"])
        .output()
        .expect("docker inspect should run");
    assert!(out.status.success(), "capsule image should still exist");

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();
}

/// `build_base_image(true)` should rebuild even when the image already exists.
#[test]
#[ignore]
fn build_base_image_rebuilds_when_rebuild_flag_set() {
    // Tag busybox as capsule so an image exists before we call rebuild.
    let _ = std::process::Command::new("docker")
        .args(["pull", "busybox:latest"])
        .output();
    std::process::Command::new("docker")
        .args(["tag", "busybox:latest", "capsule"])
        .output()
        .expect("docker tag should succeed");

    // --rebuild should trigger a fresh build (will take a while in real use).
    build_base_image(true).expect("build_base_image --rebuild should succeed");

    let out = std::process::Command::new("docker")
        .args(["image", "inspect", "capsule"])
        .output()
        .expect("docker inspect should run");
    assert!(
        out.status.success(),
        "capsule image should exist after rebuild"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();
}

// ── Integration tests: run_iteration ─────────────────────────────────────────

/// Container exits 0 → run_iteration returns Ok(()).
#[test]
#[ignore]
fn run_iteration_succeeds_on_container_exit_zero() {
    // Build a minimal stub image that just exits 0.
    let dockerfile = "FROM busybox\nENTRYPOINT [\"sh\", \"-c\", \"exit 0\"]\n";
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-test-exit0", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(dockerfile.as_bytes())
            .unwrap();
    }
    child.wait().expect("docker build should complete");

    let result = run_iteration(&RunConfig {
        image: "capsule-test-exit0".to_string(),
        prompt: "hello".to_string(),
        pwd: std::env::temp_dir(),
        capsule_dir: std::env::temp_dir(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    });
    assert!(result.is_ok(), "exit 0 should return Ok: {:?}", result);

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-exit0"])
        .output();
}

/// Container exits non-zero → run_iteration returns an error naming the exit code.
#[test]
#[ignore]
fn run_iteration_errors_on_container_exit_nonzero() {
    let dockerfile = "FROM busybox\nENTRYPOINT [\"sh\", \"-c\", \"exit 42\"]\n";
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-test-exit42", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(dockerfile.as_bytes())
            .unwrap();
    }
    child.wait().expect("docker build should complete");

    let result = run_iteration(&RunConfig {
        image: "capsule-test-exit42".to_string(),
        prompt: "hello".to_string(),
        pwd: std::env::temp_dir(),
        capsule_dir: std::env::temp_dir(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    });
    assert!(result.is_err(), "non-zero exit should return Err");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("42"),
        "error should mention exit code 42, got: {msg}"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-exit42"])
        .output();
}

/// authentication_failed in output → run_iteration returns specific error.
#[test]
#[ignore]
fn run_iteration_errors_on_auth_failure_in_output() {
    let auth_line = r#"{"type":"result","subtype":"error","error":"authentication_failed"}"#;
    let dockerfile = format!(
        "FROM busybox\nENTRYPOINT [\"sh\", \"-c\", \"printf '%s\\n' '{}'; exit 0\"]\n",
        auth_line
    );
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-test-authfail", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(dockerfile.as_bytes())
            .unwrap();
    }
    child.wait().expect("docker build should complete");

    let result = run_iteration(&RunConfig {
        image: "capsule-test-authfail".to_string(),
        prompt: "hello".to_string(),
        pwd: std::env::temp_dir(),
        capsule_dir: std::env::temp_dir(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    });
    assert!(result.is_err(), "auth failure should return Err");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.to_lowercase().contains("auth") || msg.contains("claude"),
        "error should mention auth/claude, got: {msg}"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-authfail"])
        .output();
}

/// Container output contains NO MORE TASKS marker → run_iteration returns Done.
#[test]
#[ignore]
fn run_iteration_returns_done_on_no_more_tasks_marker() {
    let marker_line =
        r#"{"type":"result","subtype":"success","result":"<promise>NO MORE TASKS</promise>"}"#;
    let dockerfile = format!(
        "FROM busybox\nENTRYPOINT [\"sh\", \"-c\", \"printf '%s\\n' '{}'; exit 0\"]\n",
        marker_line
    );
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-test-nomore", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(dockerfile.as_bytes())
            .unwrap();
    }
    child.wait().expect("docker build should complete");

    let result = run_iteration(&RunConfig {
        image: "capsule-test-nomore".to_string(),
        prompt: "hello".to_string(),
        pwd: std::env::temp_dir(),
        capsule_dir: std::env::temp_dir(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    });
    assert!(result.is_ok(), "marker should not error: {:?}", result);
    assert!(
        matches!(result.unwrap(), IterationOutcome::Done),
        "should return Done when marker present"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-nomore"])
        .output();
}

/// Container output without marker → run_iteration returns Continue.
#[test]
#[ignore]
fn run_iteration_returns_continue_without_marker() {
    let dockerfile = "FROM busybox\nENTRYPOINT [\"sh\", \"-c\", \"echo normal output; exit 0\"]\n";
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-test-continue", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(dockerfile.as_bytes())
            .unwrap();
    }
    child.wait().expect("docker build should complete");

    let result = run_iteration(&RunConfig {
        image: "capsule-test-continue".to_string(),
        prompt: "hello".to_string(),
        pwd: std::env::temp_dir(),
        capsule_dir: std::env::temp_dir(),
        model: None,
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    });
    assert!(result.is_ok(), "no marker should not error: {:?}", result);
    assert!(
        matches!(result.unwrap(), IterationOutcome::Continue),
        "should return Continue when marker absent"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-continue"])
        .output();
}

/// --model flag passes CAPSULE_MODEL env var into the container.
/// Stub image writes the env var to /workspace so we can verify it from the host.
#[test]
#[ignore]
fn run_iteration_with_model_passes_capsule_model_to_container() {
    let workdir = tempfile::tempdir().expect("temp workdir");
    let output_file = workdir.path().join("model_output.txt");

    // Entrypoint: write $CAPSULE_MODEL to /workspace/model_output.txt then exit 0.
    let dockerfile =
        "FROM busybox\nENTRYPOINT [\"sh\", \"-c\", \"echo \\\"$CAPSULE_MODEL\\\" > /workspace/model_output.txt; exit 0\"]\n";
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-test-model", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(dockerfile.as_bytes())
            .unwrap();
    }
    child.wait().expect("docker build should complete");

    let result = run_iteration(&RunConfig {
        image: "capsule-test-model".to_string(),
        prompt: "hello".to_string(),
        pwd: workdir.path().to_path_buf(),
        capsule_dir: std::env::temp_dir(),
        model: Some("claude-opus-4-6".to_string()),
        verbose: false,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    });
    assert!(result.is_ok(), "model run should not error: {:?}", result);

    let written = std::fs::read_to_string(&output_file)
        .expect("container should have written model_output.txt");
    assert!(
        written.trim() == "claude-opus-4-6",
        "container should receive CAPSULE_MODEL=claude-opus-4-6, got: {written:?}"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-model"])
        .output();
}

/// --verbose=true does not change iteration outcome; run completes normally.
#[test]
#[ignore]
fn run_iteration_with_verbose_completes_normally() {
    let dockerfile = "FROM busybox\nENTRYPOINT [\"sh\", \"-c\", \"exit 0\"]\n";
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-test-verbose", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(dockerfile.as_bytes())
            .unwrap();
    }
    child.wait().expect("docker build should complete");

    let result = run_iteration(&RunConfig {
        image: "capsule-test-verbose".to_string(),
        prompt: "hello".to_string(),
        pwd: std::env::temp_dir(),
        capsule_dir: std::env::temp_dir(),
        model: None,
        verbose: true,
        env_file: None,
        gh_token: None,
        git_author_name: "".to_string(),
        git_author_email: "".to_string(),
        before_each_path: None,
    });
    assert!(
        result.is_ok(),
        "verbose run should complete normally: {:?}",
        result
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-verbose"])
        .output();
}
