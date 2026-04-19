mod common;

use capsule::docker::{
    build_base_image, build_derived_image, build_docker_args, contains_auth_failure,
    contains_no_more_tasks, derived_image_name, detect_compose_network, run_iteration,
    IterationOutcome, RunConfig, DOCKERFILE, STREAM_DISPLAY_JQ,
};
use common::requires_docker;
use serial_test::serial;
use std::sync::{Arc, Mutex};

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

#[test]
fn no_more_tasks_detected_in_result_line() {
    let line =
        r#"{"type":"result","subtype":"success","result":"<promise>AFK_COMPLETE</promise>"}"#;
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

#[test]
fn prompt_mount_is_not_read_only() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
    let prompt_arg = args.iter().find(|a| a.contains("prompt.txt")).unwrap();
    assert!(
        !prompt_arg.ends_with(":ro"),
        "prompt.txt must not be mounted read-only so before-each.sh can mutate it: {prompt_arg}"
    );
}

#[test]
fn env_file_arg_present_when_file_exists() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join(".env"), "FOO=bar\n").unwrap();

    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        env_file: Some(dir.path().join(".env")),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        gh_token_env_file: Some(token_file.clone()),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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
fn gh_token_not_in_docker_args_when_env_file_none() {
    let dir = tempfile::tempdir().expect("temp dir");

    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
    let joined = args.join(" ");
    assert!(
        !joined.contains("GH_TOKEN"),
        "token must not appear in docker args: {joined}"
    );
}

#[test]
fn gh_token_never_appears_inline_in_docker_args() {
    // Even if a token string is known, it must not show up in the arg list directly.
    // The only valid path is via --env-file.
    let dir = tempfile::tempdir().expect("temp dir");
    let token_file = dir.path().join("gh-token.env");
    std::fs::write(&token_file, "GH_TOKEN=ghs_secret\n").unwrap();

    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        gh_token_env_file: Some(token_file),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
    for arg in &args {
        assert!(
            !arg.contains("ghs_secret"),
            "token value must not appear inline in docker arg: {arg}"
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
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        git_author_name: "Bob Builder".to_string(),
        git_author_email: "bob@example.com".to_string(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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

#[test]
fn before_each_mounted_when_path_provided() {
    let dir = tempfile::tempdir().expect("temp dir");
    let before_each = dir.path().join("before-each.sh");
    std::fs::write(&before_each, "#!/bin/sh\n").unwrap();

    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        before_each_path: Some(before_each.clone()),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
    let joined = args.join(" ");
    assert!(
        joined.contains("/home/claude/before-each.sh:ro"),
        "expected before-each.sh mount in args: {joined}"
    );
    assert!(
        joined.contains(before_each.to_string_lossy().as_ref()),
        "expected host path in mount: {joined}"
    );
}

#[test]
fn before_each_not_mounted_when_absent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
    let joined = args.join(" ");
    assert!(
        !joined.contains("before-each.sh"),
        "before-each.sh must not appear in args when path is None: {joined}"
    );
}

#[test]
fn model_arg_present_when_model_set() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        model: Some("claude-opus-4-6".to_string()),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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
        pwd: dir.path().to_path_buf(),
        verbose: true,
        ..RunConfig::default()
    };
    let cfg_quiet = RunConfig {
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args_verbose = build_docker_args(&cfg_verbose, prompt_file.path(), "capsule-test");
    let args_quiet = build_docker_args(&cfg_quiet, prompt_file.path(), "capsule-test");
    assert_eq!(
        args_verbose, args_quiet,
        "verbose flag must not alter docker args"
    );
}

#[test]
fn container_name_present_in_docker_args() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-run-12345-1");
    let joined = args.join(" ");
    assert!(
        joined.contains("--name capsule-run-12345-1"),
        "expected --name in args: {joined}"
    );
}

#[test]
fn container_name_for_has_expected_format() {
    use capsule::docker::container_name_for;
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
#[requires_docker]
#[serial]
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

#[test]
#[requires_docker]
#[serial]
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

#[test]
#[requires_docker]
#[serial]
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

#[test]
#[requires_docker]
#[serial]
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

    let result = run_iteration(
        &RunConfig {
            image: "capsule-test-exit0".to_string(),
            prompt: "hello".to_string(),
            pwd: std::env::temp_dir(),
            claude_dir: std::env::temp_dir(),
            ..RunConfig::default()
        },
        1,
        &Arc::new(Mutex::new(None)),
    );
    assert!(result.is_ok(), "exit 0 should return Ok: {:?}", result);

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-exit0"])
        .output();
}

#[test]
#[requires_docker]
#[serial]
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

    let result = run_iteration(
        &RunConfig {
            image: "capsule-test-exit42".to_string(),
            prompt: "hello".to_string(),
            pwd: std::env::temp_dir(),
            claude_dir: std::env::temp_dir(),
            ..RunConfig::default()
        },
        1,
        &Arc::new(Mutex::new(None)),
    );
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

#[test]
#[requires_docker]
#[serial]
fn run_iteration_errors_on_auth_failure_in_output() {
    // Bake the JSON line into the image with RUN to avoid ENTRYPOINT JSON escaping issues.
    let dockerfile =
        "FROM busybox\nRUN echo '{\"type\":\"result\",\"subtype\":\"error\",\"error\":\"authentication_failed\"}' > /out.txt\nENTRYPOINT [\"cat\", \"/out.txt\"]\n";
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

    let result = run_iteration(
        &RunConfig {
            image: "capsule-test-authfail".to_string(),

            prompt: "hello".to_string(),
            pwd: std::env::temp_dir(),
            claude_dir: std::env::temp_dir(),
            ..RunConfig::default()
        },
        1,
        &Arc::new(Mutex::new(None)),
    );
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

#[test]
#[requires_docker]
#[serial]
fn run_iteration_returns_done_on_no_more_tasks_marker() {
    // Bake the JSON line into the image with RUN to avoid ENTRYPOINT JSON escaping issues.
    let dockerfile =
        "FROM busybox\nRUN echo '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"<promise>AFK_COMPLETE</promise>\"}' > /out.txt\nENTRYPOINT [\"cat\", \"/out.txt\"]\n";
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

    let result = run_iteration(
        &RunConfig {
            image: "capsule-test-nomore".to_string(),
            prompt: "hello".to_string(),
            pwd: std::env::temp_dir(),
            claude_dir: std::env::temp_dir(),
            ..RunConfig::default()
        },
        1,
        &Arc::new(Mutex::new(None)),
    );
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

#[test]
#[requires_docker]
#[serial]
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

    let result = run_iteration(
        &RunConfig {
            image: "capsule-test-continue".to_string(),
            prompt: "hello".to_string(),
            pwd: std::env::temp_dir(),
            claude_dir: std::env::temp_dir(),
            ..RunConfig::default()
        },
        1,
        &Arc::new(Mutex::new(None)),
    );
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

#[test]
#[requires_docker]
#[serial]
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

    let result = run_iteration(
        &RunConfig {
            image: "capsule-test-model".to_string(),
            prompt: "hello".to_string(),
            pwd: workdir.path().to_path_buf(),
            model: Some("claude-opus-4-6".to_string()),
            claude_dir: std::env::temp_dir(),
            ..RunConfig::default()
        },
        1,
        &Arc::new(Mutex::new(None)),
    );
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

#[test]
#[requires_docker]
#[serial]
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

    let result = run_iteration(
        &RunConfig {
            image: "capsule-test-verbose".to_string(),
            prompt: "hello".to_string(),
            pwd: std::env::temp_dir(),
            verbose: true,
            claude_dir: std::env::temp_dir(),
            ..RunConfig::default()
        },
        1,
        &Arc::new(Mutex::new(None)),
    );
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

#[test]
fn compose_network_arg_present_when_set() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        compose_network: Some("myproject_default".to_string()),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
    let joined = args.join(" ");
    assert!(
        !joined.contains("--network"),
        "expected no --network when compose_network is None: {joined}"
    );
}

#[test]
fn claude_dir_mounted_at_home_claude_dot_claude() {
    let dir = tempfile::tempdir().expect("temp dir");
    let claude_dir = tempfile::tempdir().expect("claude temp dir");
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    let cfg = RunConfig {
        pwd: dir.path().to_path_buf(),
        claude_dir: claude_dir.path().to_path_buf(),
        ..RunConfig::default()
    };
    let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
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

#[test]
fn detect_compose_network_returns_none_when_no_project() {
    let dir = tempfile::tempdir().expect("temp dir");
    let result = detect_compose_network(dir.path());
    assert!(
        result.is_none(),
        "expected None when no compose project running at temp dir"
    );
}

#[test]
#[requires_docker]
fn detect_compose_network_returns_network_for_running_project() {
    let dir = tempfile::tempdir().expect("temp dir");
    let compose_file = dir.path().join("docker-compose.yml");
    std::fs::write(
        &compose_file,
        "services:\n  web:\n    image: busybox\n    command: sleep 30\n",
    )
    .unwrap();

    // Start compose project.
    std::process::Command::new("docker")
        .args(["compose", "-f", &compose_file.to_string_lossy(), "up", "-d"])
        .current_dir(dir.path())
        .output()
        .expect("docker compose up should run");

    // Give it a moment to start.
    std::thread::sleep(std::time::Duration::from_secs(2));

    let result = detect_compose_network(dir.path());
    assert!(
        result.is_some(),
        "expected a network name for running compose project"
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["compose", "-f", &compose_file.to_string_lossy(), "down"])
        .current_dir(dir.path())
        .output();
}

#[test]
fn derived_image_name_uses_basename_of_pwd() {
    let dir = tempfile::tempdir().expect("temp dir");
    let project_dir = dir.path().join("my-project");
    std::fs::create_dir(&project_dir).unwrap();
    let name = derived_image_name(&project_dir);
    assert_eq!(name, "capsule-my-project");
}

#[test]
fn derived_image_name_handles_root_or_unnamed() {
    // If basename is empty (unlikely in practice), should not panic.
    let name = derived_image_name(std::path::Path::new("/"));
    assert!(name.starts_with("capsule-"), "name={name}");
}

#[test]
fn build_derived_image_returns_none_when_no_dockerfile() {
    let capsule_dir = tempfile::tempdir().expect("temp dir");
    let pwd = tempfile::tempdir().expect("temp dir");
    // No Dockerfile in capsule_dir → returns None without touching Docker.
    let result = build_derived_image(capsule_dir.path(), pwd.path(), false)
        .expect("should not error when Dockerfile absent");
    assert!(result.is_none(), "expected None when no Dockerfile");
}

#[test]
#[requires_docker]
fn build_derived_image_builds_and_returns_image_name() {
    let capsule_dir = tempfile::tempdir().expect("temp dir");
    let base = tempfile::tempdir().expect("temp dir");
    let pwd = base.path().join("myproject");
    std::fs::create_dir(&pwd).unwrap();

    // Write a minimal Dockerfile. Uses busybox so the test doesn't require the
    // capsule base image to be pre-built.
    std::fs::write(
        capsule_dir.path().join("Dockerfile"),
        "FROM busybox\nRUN echo derived\n",
    )
    .unwrap();

    let name = build_derived_image(capsule_dir.path(), &pwd, false)
        .expect("build_derived_image should succeed")
        .expect("expected Some(name) when Dockerfile present");

    assert!(
        name.starts_with("capsule-"),
        "derived image name should start with capsule-: {name}"
    );

    // Cleanup: remove the derived image.
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", &name])
        .output();
}

#[test]
#[requires_docker]
fn build_derived_image_skips_build_when_image_exists_and_no_rebuild() {
    let capsule_dir = tempfile::tempdir().expect("temp dir");
    let base = tempfile::tempdir().expect("temp dir");
    let pwd = base.path().join("myproject");
    std::fs::create_dir(&pwd).unwrap();

    std::fs::write(
        capsule_dir.path().join("Dockerfile"),
        "FROM busybox\nRUN echo derived\n",
    )
    .unwrap();

    // First build.
    let name = build_derived_image(capsule_dir.path(), &pwd, false)
        .unwrap()
        .unwrap();

    // Second call with rebuild=false — should succeed without rebuilding.
    let name2 = build_derived_image(capsule_dir.path(), &pwd, false)
        .unwrap()
        .unwrap();
    assert_eq!(name, name2);

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", &name])
        .output();
}

#[test]
#[requires_docker]
#[serial]
fn build_base_image_stores_hash_label() {
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();

    build_base_image(false).expect("build_base_image should succeed");

    let out = std::process::Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            r#"{{index .Config.Labels "capsule.dockerfile.hash"}}"#,
            "capsule",
        ])
        .output()
        .expect("docker inspect should run");

    let label = String::from_utf8(out.stdout).unwrap();
    let label = label.trim();
    assert!(
        !label.is_empty(),
        "capsule.dockerfile.hash label should be set"
    );
    assert_eq!(label.len(), 16, "hash should be a 16-char hex string");

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();
}

#[test]
#[requires_docker]
fn build_derived_image_stores_hash_label() {
    let capsule_dir = tempfile::tempdir().expect("temp dir");
    let base = tempfile::tempdir().expect("temp dir");
    let pwd = base.path().join("hashtest");
    std::fs::create_dir(&pwd).unwrap();

    std::fs::write(
        capsule_dir.path().join("Dockerfile"),
        "FROM busybox\nRUN echo hashtest\n",
    )
    .unwrap();

    let name = build_derived_image(capsule_dir.path(), &pwd, false)
        .unwrap()
        .unwrap();

    let out = std::process::Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            r#"{{index .Config.Labels "capsule.dockerfile.hash"}}"#,
            &name,
        ])
        .output()
        .expect("docker inspect should run");

    let label = String::from_utf8(out.stdout).unwrap();
    let label = label.trim();
    assert!(
        !label.is_empty(),
        "capsule.dockerfile.hash label should be set on derived image"
    );
    assert_eq!(label.len(), 16, "hash should be a 16-char hex string");

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", &name])
        .output();
}

#[test]
#[requires_docker]
fn build_derived_image_rebuilds_when_dockerfile_changes() {
    let capsule_dir = tempfile::tempdir().expect("temp dir");
    let base = tempfile::tempdir().expect("temp dir");
    let pwd = base.path().join("changetest");
    std::fs::create_dir(&pwd).unwrap();

    let dockerfile_path = capsule_dir.path().join("Dockerfile");
    std::fs::write(&dockerfile_path, "FROM busybox\nRUN echo version1\n").unwrap();

    let name = build_derived_image(capsule_dir.path(), &pwd, false)
        .unwrap()
        .unwrap();

    // Record hash from first build.
    let out1 = std::process::Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            r#"{{index .Config.Labels "capsule.dockerfile.hash"}}"#,
            &name,
        ])
        .output()
        .unwrap();
    let hash1 = String::from_utf8(out1.stdout).unwrap().trim().to_owned();

    // Change the Dockerfile content.
    std::fs::write(&dockerfile_path, "FROM busybox\nRUN echo version2\n").unwrap();

    // Should auto-rebuild without --rebuild flag.
    let name2 = build_derived_image(capsule_dir.path(), &pwd, false)
        .unwrap()
        .unwrap();
    assert_eq!(name, name2, "image name should be unchanged");

    let out2 = std::process::Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            r#"{{index .Config.Labels "capsule.dockerfile.hash"}}"#,
            &name2,
        ])
        .output()
        .unwrap();
    let hash2 = String::from_utf8(out2.stdout).unwrap().trim().to_owned();

    assert_ne!(
        hash1, hash2,
        "hash label should update after Dockerfile change"
    );

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", &name])
        .output();
}
