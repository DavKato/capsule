mod common;

use capsule::container_execution::{detect_compose_network, run_iteration, ExecutionConfig};
use capsule::image_build::{build_base_image, build_derived_image, BuildConfig};
use common::requires_docker;
use serial_test::serial;
use std::io::Write;
use std::sync::{Arc, Mutex};

const STUB_CLAUDE_DOCKERFILE: &str = "FROM capsule\n\
    RUN printf '#!/bin/sh\\nexit 0\\n' > /home/claude/.local/bin/claude \
    && chmod +x /home/claude/.local/bin/claude\n";

fn build_stub_capsule_image(tag: &str) {
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", tag, "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(STUB_CLAUDE_DOCKERFILE.as_bytes())
        .unwrap();
    child.wait().expect("docker build should complete");
}

fn run_entrypoint_cmd(tag: &str, env: &[(&str, &str)], shell_cmd: &str) -> std::process::Output {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt = dir.path().join("prompt.txt");
    std::fs::write(&prompt, "placeholder\n").unwrap();

    let vol = format!("{}:/home/claude/prompt.txt", prompt.display());
    let full_cmd = format!("bash /home/claude/entrypoint.sh && {shell_cmd}");

    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "--entrypoint".into(),
        "bash".into(),
        "-v".into(),
        vol,
    ];
    for (k, v) in env {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    args.push(tag.into());
    args.push("-c".into());
    args.push(full_cmd);

    std::process::Command::new("docker")
        .args(&args)
        .output()
        .expect("docker run should succeed")
}

fn cleanup_image(tag: &str) {
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", tag])
        .output();
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

    std::process::Command::new("docker")
        .args(["compose", "-f", &compose_file.to_string_lossy(), "up", "-d"])
        .current_dir(dir.path())
        .output()
        .expect("docker compose up should run");

    std::thread::sleep(std::time::Duration::from_secs(2));

    let result = detect_compose_network(dir.path());
    assert!(
        result.is_some(),
        "expected a network name for running compose project"
    );

    let _ = std::process::Command::new("docker")
        .args(["compose", "-f", &compose_file.to_string_lossy(), "down"])
        .current_dir(dir.path())
        .output();
}

#[test]
#[requires_docker]
#[serial(run_iteration)]
fn run_iteration_with_model_passes_capsule_model_to_container() {
    let workdir = tempfile::tempdir().expect("temp workdir");
    let output_file = workdir.path().join("model_output.txt");

    let dockerfile =
        "FROM busybox\nENTRYPOINT [\"sh\", \"-c\", \"echo \\\"$CAPSULE_MODEL\\\" > \\\"$CAPSULE_WORKSPACE/model_output.txt\\\"; exit 0\"]\n";
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
        &ExecutionConfig {
            image: "capsule-test-model".to_string(),
            prompt: "hello".to_string(),
            pwd: workdir.path().to_path_buf(),
            model: Some("claude-opus-4-6".to_string()),
            claude_dir: std::env::temp_dir(),
            ..ExecutionConfig::default()
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

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-model"])
        .output();
}

#[test]
#[requires_docker]
fn build_derived_image_builds_and_returns_image_name() {
    let capsule_dir = tempfile::tempdir().expect("temp dir");
    let base = tempfile::tempdir().expect("temp dir");
    let pwd = base.path().join("myproject");
    std::fs::create_dir(&pwd).unwrap();

    std::fs::write(
        capsule_dir.path().join("Dockerfile"),
        "FROM busybox\nRUN echo derived\n",
    )
    .unwrap();

    let name = build_derived_image(&BuildConfig {
        rebuild: false,
        capsule_dir: capsule_dir.path().to_path_buf(),
        pwd: pwd.clone(),
    })
    .expect("build_derived_image should succeed")
    .expect("expected Some(name) when Dockerfile present");

    assert!(
        name.starts_with("capsule-"),
        "derived image name should start with capsule-: {name}"
    );

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

    let build_cfg = BuildConfig {
        rebuild: false,
        capsule_dir: capsule_dir.path().to_path_buf(),
        pwd: pwd.clone(),
    };
    let name = build_derived_image(&build_cfg).unwrap().unwrap();
    let name2 = build_derived_image(&build_cfg).unwrap().unwrap();
    assert_eq!(name, name2);

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", &name])
        .output();
}

#[test]
#[requires_docker]
#[serial(base_image)]
fn build_base_image_skips_rebuild_when_hash_matches() {
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
        .output();

    build_base_image(false).expect("first build should succeed");

    let id1 = std::process::Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", "capsule"])
        .output()
        .expect("docker inspect should run");
    let id1 = String::from_utf8(id1.stdout).unwrap().trim().to_owned();

    build_base_image(false).expect("second build should succeed");

    let id2 = std::process::Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", "capsule"])
        .output()
        .expect("docker inspect should run");
    let id2 = String::from_utf8(id2.stdout).unwrap().trim().to_owned();

    assert_eq!(id1, id2, "image should not be rebuilt when hash matches");

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule"])
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

    let name = build_derived_image(&BuildConfig {
        rebuild: false,
        capsule_dir: capsule_dir.path().to_path_buf(),
        pwd: pwd.clone(),
    })
    .unwrap()
    .unwrap();

    let id1_out = std::process::Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", &name])
        .output()
        .unwrap();
    let id1 = String::from_utf8(id1_out.stdout).unwrap().trim().to_owned();

    std::fs::write(&dockerfile_path, "FROM busybox\nRUN echo version2\n").unwrap();

    let name2 = build_derived_image(&BuildConfig {
        rebuild: false,
        capsule_dir: capsule_dir.path().to_path_buf(),
        pwd: pwd.clone(),
    })
    .unwrap()
    .unwrap();
    assert_eq!(name, name2, "image name should be unchanged");

    let id2_out = std::process::Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", &name2])
        .output()
        .unwrap();
    let id2 = String::from_utf8(id2_out.stdout).unwrap().trim().to_owned();

    assert_ne!(id1, id2, "image should be rebuilt after Dockerfile change");

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", &name])
        .output();
}

#[test]
#[requires_docker]
fn mcp_serve_handles_initialize_and_submit_verdict_in_container() {
    use std::io::{BufRead, BufReader, Write};

    let capsule_bin = assert_cmd::cargo::cargo_bin("capsule");

    let _ = std::process::Command::new("docker")
        .args(["pull", "--quiet", "archlinux:base"])
        .output();

    let mut child = std::process::Command::new("docker")
        .args([
            "run",
            "--rm",
            "-i",
            &format!("-v={}:/usr/local/bin/capsule:ro", capsule_bin.display()),
            "archlinux:base",
            "/usr/local/bin/capsule",
            "mcp-serve",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn docker run capsule mcp-serve");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":0,"method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"test","version":"0.1"}}}}}}"#
    )
    .unwrap();
    let mut init_resp = String::new();
    reader.read_line(&mut init_resp).unwrap();
    let init_v: serde_json::Value = serde_json::from_str(init_resp.trim()).unwrap();
    assert_eq!(init_v["result"]["protocolVersion"], "2024-11-05");

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"submit_verdict","arguments":{{"status":"pass","notes":"smoke test"}}}}}}"#
    )
    .unwrap();
    let mut call_resp = String::new();
    reader.read_line(&mut call_resp).unwrap();
    let call_v: serde_json::Value = serde_json::from_str(call_resp.trim()).unwrap();
    let text = call_v["result"]["content"][0]["text"].as_str().unwrap();
    let inner: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(inner["ok"], true);
    assert_eq!(inner["verdict"]["status"], "pass");
    assert_eq!(inner["verdict"]["notes"], "smoke test");

    drop(stdin);
    let _ = child.wait();
}

#[test]
#[requires_docker]
#[serial(base_image)]
fn entrypoint_runs_capsule_stage_setup_when_env_set() {
    build_base_image(false).expect("base image should be available");
    build_stub_capsule_image("capsule-stage-setup-test");

    let dir = tempfile::tempdir().expect("temp dir");

    // Script without executable bit — exercises the bash -c invocation.
    let setup_script = dir.path().join("stage-setup.sh");
    std::fs::write(
        &setup_script,
        "#!/bin/bash\necho SENTINEL >> /home/claude/prompt.txt\n",
    )
    .unwrap();

    let prompt = dir.path().join("prompt.txt");
    std::fs::write(&prompt, "ORIGINAL\n").unwrap();
    // Workaround: container runs as uid 1000 which may differ from the host uid.
    // Drop once `docker run` passes `--user $(id -u):$(id -g)`.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&prompt, std::fs::Permissions::from_mode(0o666)).unwrap();
    }

    let _output = std::process::Command::new("docker")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{}:/home/claude/stage-setup.sh:ro", setup_script.display()),
            "-v",
            &format!("{}:/home/claude/prompt.txt", prompt.display()),
            "-e",
            "GIT_AUTHOR_NAME=Test",
            "-e",
            "GIT_AUTHOR_EMAIL=test@test.com",
            "-e",
            "CAPSULE_STAGE_SETUP=/home/claude/stage-setup.sh",
            "capsule-stage-setup-test",
        ])
        .output()
        .expect("docker run should succeed");

    let contents =
        std::fs::read_to_string(&prompt).expect("prompt.txt should be readable after run");
    assert!(
        contents.contains("SENTINEL"),
        "CAPSULE_STAGE_SETUP script should have appended SENTINEL to prompt.txt: {contents:?}"
    );

    cleanup_image("capsule-stage-setup-test");
}

#[test]
#[requires_docker]
#[serial(base_image)]
fn entrypoint_runs_capsule_stage_setup_inline_command() {
    build_base_image(false).expect("base image should be available");
    build_stub_capsule_image("capsule-stage-setup-inline-test");

    let dir = tempfile::tempdir().expect("temp dir");
    let prompt = dir.path().join("prompt.txt");
    std::fs::write(&prompt, "ORIGINAL\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&prompt, std::fs::Permissions::from_mode(0o666)).unwrap();
    }

    let _output = std::process::Command::new("docker")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{}:/home/claude/prompt.txt", prompt.display()),
            "-e",
            "GIT_AUTHOR_NAME=Test",
            "-e",
            "GIT_AUTHOR_EMAIL=test@test.com",
            "-e",
            "CAPSULE_STAGE_SETUP=echo INLINE >> /home/claude/prompt.txt",
            "capsule-stage-setup-inline-test",
        ])
        .output()
        .expect("docker run should succeed");

    let contents =
        std::fs::read_to_string(&prompt).expect("prompt.txt should be readable after run");
    assert!(
        contents.contains("INLINE"),
        "CAPSULE_STAGE_SETUP inline command should have appended INLINE to prompt.txt: {contents:?}"
    );

    cleanup_image("capsule-stage-setup-inline-test");
}

#[test]
#[requires_docker]
#[serial(base_image)]
fn entrypoint_uses_resume_when_env_set() {
    build_base_image(false).expect("base image should be available");

    // Stub claude that logs its args to /tmp/claude-args.
    let dockerfile = "FROM capsule\n\
        RUN printf '#!/bin/sh\\necho \"$@\" > /tmp/claude-args\\n' > /home/claude/.local/bin/claude \
        && chmod +x /home/claude/.local/bin/claude\n";
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-resume-test", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(dockerfile.as_bytes())
        .unwrap();
    child.wait().expect("docker build should complete");

    let dir = tempfile::tempdir().expect("temp dir");
    let prompt = dir.path().join("prompt.txt");
    std::fs::write(&prompt, "placeholder\n").unwrap();

    let output = std::process::Command::new("docker")
        .args([
            "run",
            "--rm",
            "--entrypoint",
            "bash",
            "-v",
            &format!("{}:/home/claude/prompt.txt", prompt.display()),
            "-e",
            "GIT_AUTHOR_NAME=Test",
            "-e",
            "GIT_AUTHOR_EMAIL=test@test.com",
            "-e",
            "CAPSULE_RESUME_SESSION=sess_xyz",
            "capsule-resume-test",
            "-c",
            "bash /home/claude/entrypoint.sh && cat /tmp/claude-args",
        ])
        .output()
        .expect("docker run should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("--resume sess_xyz"),
        "expected --resume sess_xyz in claude args.\nstdout: {stdout}\nstderr: {stderr}"
    );

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-resume-test"])
        .output();
}

#[test]
#[requires_docker]
#[serial(base_image)]
fn git_wrapper_enforces_identity_over_agent_override() {
    build_base_image(false).expect("base image should be available");
    build_stub_capsule_image("capsule-git-wrapper-test");

    let output = run_entrypoint_cmd(
        "capsule-git-wrapper-test",
        &[
            ("GIT_AUTHOR_NAME", "Capsule"),
            ("GIT_AUTHOR_EMAIL", "capsule@localhost"),
        ],
        concat!(
            "cd /tmp && git init testrepo && cd testrepo && ",
            "GIT_AUTHOR_NAME=Agent GIT_AUTHOR_EMAIL=agent@fake.com ",
            "GIT_COMMITTER_NAME=Agent GIT_COMMITTER_EMAIL=agent@fake.com ",
            "git commit --allow-empty -m test && ",
            "git log -1 --format='%an <%ae>'"
        ),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("Capsule <capsule@localhost>"),
        "wrapper should enforce capsule identity over agent override.\nstdout: {stdout}\nstderr: {stderr}"
    );

    cleanup_image("capsule-git-wrapper-test");
}

#[test]
#[requires_docker]
#[serial(base_image)]
fn git_wrapper_prevents_git_config_override() {
    build_base_image(false).expect("base image should be available");
    build_stub_capsule_image("capsule-git-config-test");

    let output = run_entrypoint_cmd(
        "capsule-git-config-test",
        &[
            ("GIT_AUTHOR_NAME", "Capsule"),
            ("GIT_AUTHOR_EMAIL", "capsule@localhost"),
        ],
        concat!(
            "cd /tmp && git init testrepo && cd testrepo && ",
            "git config --global user.name 'Config Hacker' && ",
            "git config --global user.email 'hacker@fake.com' && ",
            "git commit --allow-empty -m test && ",
            "git log -1 --format='%an <%ae>'"
        ),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("Capsule <capsule@localhost>"),
        "wrapper should enforce capsule identity even when git config is set.\nstdout: {stdout}\nstderr: {stderr}"
    );

    cleanup_image("capsule-git-config-test");
}

#[test]
#[requires_docker]
#[serial(base_image)]
fn user_identity_passthrough_works_correctly() {
    build_base_image(false).expect("base image should be available");
    build_stub_capsule_image("capsule-user-identity-test");

    let output = run_entrypoint_cmd(
        "capsule-user-identity-test",
        &[
            ("GIT_AUTHOR_NAME", "Alice Developer"),
            ("GIT_AUTHOR_EMAIL", "alice@example.com"),
        ],
        concat!(
            "cd /tmp && git init usertest && cd usertest && ",
            "git commit --allow-empty -m 'test' && ",
            "git log -1 --format='%an <%ae>'"
        ),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("Alice Developer <alice@example.com>"),
        "user identity should be passed through and used for commits.\nstdout: {stdout}\nstderr: {stderr}"
    );

    cleanup_image("capsule-user-identity-test");
}

#[test]
#[requires_docker]
#[serial(base_image)]
fn direct_git_binary_still_uses_capsule_identity() {
    build_base_image(false).expect("base image should be available");
    build_stub_capsule_image("capsule-bypass-test");

    let dir = tempfile::tempdir().expect("temp dir");
    let prompt = dir.path().join("prompt.txt");
    std::fs::write(&prompt, "placeholder\n").unwrap();

    let output = std::process::Command::new("docker")
        .args([
            "run",
            "--rm",
            "--entrypoint",
            "bash",
            "-v",
            &format!("{}:/home/claude/prompt.txt", prompt.display()),
            "-e",
            "GIT_AUTHOR_NAME=Capsule",
            "-e",
            "GIT_AUTHOR_EMAIL=capsule@localhost",
            "capsule-bypass-test",
            "-c",
            concat!(
                ". /home/claude/entrypoint.sh 2>&1 > /dev/null || true; ",
                "cd /tmp && git init bypasstest && cd bypasstest && ",
                "/usr/bin/git commit --allow-empty -m 'test' && ",
                "/usr/bin/git log -1 --format='%an <%ae>'"
            ),
        ])
        .output()
        .expect("docker run should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Capsule <capsule@localhost>"),
        "even when using absolute /usr/bin/git, the wrapper identity should be used because the identity env vars are still set.\nstdout: {stdout}"
    );

    cleanup_image("capsule-bypass-test");
}

/// Verify that extra_env_file values are visible in every container invocation.
/// Two simulated "stages" (consecutive run_iteration calls) must both see the
/// env var injected via extra_env_file, confirming persistence across stages.
#[test]
#[requires_docker]
#[serial(run_iteration)]
fn extra_env_file_visible_across_stages() {
    let workdir = tempfile::tempdir().expect("temp workdir");

    // Build a minimal image whose entrypoint writes the env var value to a file.
    let dockerfile =
        "FROM busybox\nENTRYPOINT [\"sh\", \"-c\", \"echo \\\"$MY_RUN_PARAM\\\" >> \\\"$CAPSULE_WORKSPACE/stage_outputs.txt\\\"; exit 0\"]\n";
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-test-extra-env", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(dockerfile.as_bytes())
            .unwrap();
    }
    child.wait().expect("docker build should complete");

    // Write the --env pairs to a temp env-file (as RunSession does).
    let mut extra_env_tmp = tempfile::Builder::new()
        .prefix("capsule-env-")
        .suffix(".env")
        .tempfile()
        .unwrap();
    writeln!(extra_env_tmp, "MY_RUN_PARAM=expected_value").unwrap();

    let cfg = ExecutionConfig {
        image: "capsule-test-extra-env".to_string(),
        prompt: "ignored".to_string(),
        pwd: workdir.path().to_path_buf(),
        extra_env_file: Some(extra_env_tmp.path().to_path_buf()),
        claude_dir: std::env::temp_dir(),
        ..ExecutionConfig::default()
    };
    let active = Arc::new(Mutex::new(None));

    // Stage 1
    let r1 = run_iteration(&cfg, 1, &active);
    assert!(r1.is_ok(), "stage 1 should not error: {:?}", r1);

    // Stage 2
    let r2 = run_iteration(&cfg, 2, &active);
    assert!(r2.is_ok(), "stage 2 should not error: {:?}", r2);

    let outputs = std::fs::read_to_string(workdir.path().join("stage_outputs.txt"))
        .expect("container should have written stage_outputs.txt");

    let lines: Vec<&str> = outputs.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "both stages must have written output; got: {outputs:?}"
    );
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(
            *line,
            "expected_value",
            "stage {} must see MY_RUN_PARAM=expected_value; got: {line:?}",
            i + 1
        );
    }

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-extra-env"])
        .output();
}

/// Verify that a pre-merged extra_env_file delivers all three expected values to the container.
/// Constructs the merged env ([MODE=wet, TASK=42, REGION=us]) inline and confirms the container
/// sees it via extra_env_file. The merge algorithm itself is tested by unit tests in src/run.rs.
#[test]
#[requires_docker]
#[serial(run_iteration)]
fn extra_env_file_delivers_merged_env_to_container() {
    use capsule::container_execution::ExecutionConfig;
    use std::io::Write;

    let workdir = tempfile::tempdir().expect("temp workdir");
    let output_file = workdir.path().join("env_check.txt");

    let dockerfile = "FROM busybox\n\
        ENTRYPOINT [\"sh\", \"-c\", \
        \"echo MODE=$MODE,TASK=$TASK,REGION=$REGION > \\\"$CAPSULE_WORKSPACE/env_check.txt\\\"; exit 0\"]\n";
    let mut child = std::process::Command::new("docker")
        .args(["build", "-t", "capsule-test-resume-env-merge", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("docker build should spawn");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(dockerfile.as_bytes())
        .unwrap();
    child.wait().expect("docker build should complete");

    // Simulate what prepare_resume does: merge persisted pairs with CLI overrides,
    // then write the merged result to a single extra env-file for the container.
    let persisted = vec![
        ("MODE".to_string(), "dry".to_string()),
        ("TASK".to_string(), "42".to_string()),
    ];
    let cli_overrides = vec![
        ("MODE".to_string(), "wet".to_string()),
        ("REGION".to_string(), "us".to_string()),
    ];
    // Inline the expected merged result; merge logic correctness is covered by unit tests.
    let mut merged = persisted.clone();
    for (k, v) in &cli_overrides {
        if let Some(e) = merged.iter_mut().find(|(ek, _)| ek == k) {
            e.1 = v.clone();
        } else {
            merged.push((k.clone(), v.clone()));
        }
    }
    let mut extra_env_tmp = tempfile::Builder::new()
        .prefix("capsule-env-")
        .suffix(".env")
        .tempfile()
        .unwrap();
    for (k, v) in &merged {
        writeln!(extra_env_tmp, "{k}={v}").unwrap();
    }

    let cfg = ExecutionConfig {
        image: "capsule-test-resume-env-merge".to_string(),
        prompt: "ignored".to_string(),
        pwd: workdir.path().to_path_buf(),
        extra_env_file: Some(extra_env_tmp.path().to_path_buf()),
        claude_dir: std::env::temp_dir(),
        ..ExecutionConfig::default()
    };
    let active = Arc::new(Mutex::new(None));

    let result = run_iteration(&cfg, 1, &active);
    assert!(result.is_ok(), "container run should succeed: {:?}", result);

    let output =
        std::fs::read_to_string(&output_file).expect("container should have written env_check.txt");
    assert!(
        output.trim() == "MODE=wet,TASK=42,REGION=us",
        "container must see merged env values; got: {output:?}"
    );

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "capsule-test-resume-env-merge"])
        .output();
}
