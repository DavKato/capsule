mod common;

use capsule::docker::{
    build_base_image, build_derived_image, detect_compose_network, run_iteration, RunConfig,
};
use common::requires_docker;
use serial_test::serial;
use std::sync::{Arc, Mutex};

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

    let name = build_derived_image(capsule_dir.path(), &pwd, false)
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

    let name = build_derived_image(capsule_dir.path(), &pwd, false)
        .unwrap()
        .unwrap();
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
#[serial(base_image)]
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

    let id1_out = std::process::Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", &name])
        .output()
        .unwrap();
    let id1 = String::from_utf8(id1_out.stdout).unwrap().trim().to_owned();

    std::fs::write(&dockerfile_path, "FROM busybox\nRUN echo version2\n").unwrap();

    let name2 = build_derived_image(capsule_dir.path(), &pwd, false)
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
