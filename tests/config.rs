use capsule::config::{
    resolve, CliOverrides, Config, GitIdentity, GithubScope, OnFail, OnPass, PipelineEntry,
    MAX_PIPELINE_ITERATIONS_DEFAULT,
};
use tempfile::TempDir;

fn no_cli() -> CliOverrides {
    CliOverrides::default()
}

/// Helper: create a temp capsule dir with a config.yml containing the given YAML.
fn capsule_dir_with_config(yaml: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.yml"), yaml).unwrap();
    dir
}

#[test]
fn no_config_file_uses_defaults_and_cli() {
    let dir = tempfile::tempdir().unwrap();
    let cli = CliOverrides {
        iterations: Some(3),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert_eq!(cfg.iterations, 3);
    assert!(!cfg.rebuild);
    assert!(!cfg.verbose);
    assert_eq!(cfg.git_identity, GitIdentity::User);
}

#[test]
fn config_file_iterations_used_when_no_cli_flag() {
    let dir = capsule_dir_with_config("iterations: 5\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.iterations, 5);
}

#[test]
fn cli_flag_overrides_config_file() {
    let dir = capsule_dir_with_config("iterations: 5\n");
    let cli = CliOverrides {
        iterations: Some(20),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert_eq!(cfg.iterations, 20);
}

#[test]
fn missing_config_file_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    assert!(resolve(dir.path(), cli).is_ok());
}

#[test]
fn malformed_yaml_produces_clear_error() {
    let dir = capsule_dir_with_config(": this is not valid yaml: {\n");
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    let err = resolve(dir.path(), cli).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("config.yml"),
        "error message should name the file; got: {msg}"
    );
}

#[test]
fn config_file_model_and_verbose() {
    let dir = capsule_dir_with_config("iterations: 1\nmodel: claude-opus-4-6\nverbose: true\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("claude-opus-4-6"));
    assert!(cfg.verbose);
}

#[test]
fn git_identity_capsule_from_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngit_identity: capsule\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.git_identity, GitIdentity::Capsule);
}

#[test]
fn github_absent_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert!(cfg.github.is_none());
}

#[test]
fn github_local_from_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: local\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Local));
}

#[test]
fn github_global_from_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: global\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Global));
}

#[test]
fn unknown_field_in_config_produces_clear_error() {
    let dir = capsule_dir_with_config("iterations: 1\niteraions: 5\n");
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    let err = resolve(dir.path(), cli).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("iteraions"),
        "error chain should name the unknown field; got: {chain}"
    );
}

#[test]
fn removed_rebuild_key_produces_clear_error() {
    let dir = capsule_dir_with_config("iterations: 1\nrebuild: true\n");
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    let err = resolve(dir.path(), cli).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("rebuild"),
        "error chain should name the removed field; got: {chain}"
    );
}

#[test]
fn github_cli_overrides_config() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: global\n");
    let cli = CliOverrides {
        iterations: Some(1),
        github: Some(GithubScope::Local),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Local));
}

// ── Flat-form desugar tests ───────────────────────────────────────────────────

#[test]
fn flat_form_desugars_to_single_stage_loop() {
    let dir = capsule_dir_with_config("iterations: 3\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.pipeline.entries.len(), 1);
    let PipelineEntry::Loop(ref lp) = cfg.pipeline.entries[0] else {
        panic!("expected Loop entry");
    };
    assert_eq!(lp.max_iteration, Some(3));
    assert_eq!(lp.stages.len(), 1);
    assert_eq!(lp.stages[0].name, "main");
}

#[test]
fn flat_form_desugar_has_default_max_pipeline_iterations() {
    let dir = capsule_dir_with_config("iterations: 1\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(
        cfg.pipeline.max_pipeline_iterations,
        MAX_PIPELINE_ITERATIONS_DEFAULT
    );
}

// ── Multi-stage parsing tests ─────────────────────────────────────────────────

const MULTI_STAGE_YAML: &str = "\
stages:
  - name: implementer
    prompt: prompts/implement.md
    on_fail: retry
    max_retries: 3
  - name: reviewer
    prompt: prompts/review.md
    on_fail: implementer
max_pipeline_iterations: 500
";

#[test]
fn multi_stage_parses_stages_and_routing() {
    let dir = capsule_dir_with_config(MULTI_STAGE_YAML);
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.pipeline.max_pipeline_iterations, 500);
    assert_eq!(cfg.pipeline.entries.len(), 2);

    let PipelineEntry::Stage(ref impl_stage) = cfg.pipeline.entries[0] else {
        panic!("expected Stage entry");
    };
    assert_eq!(impl_stage.name, "implementer");
    assert_eq!(impl_stage.on_fail, OnFail::Retry);
    assert_eq!(impl_stage.max_retries, Some(3));

    let PipelineEntry::Stage(ref rev_stage) = cfg.pipeline.entries[1] else {
        panic!("expected Stage entry");
    };
    assert_eq!(rev_stage.name, "reviewer");
    assert_eq!(rev_stage.on_fail, OnFail::Stage("implementer".to_string()));
    assert_eq!(rev_stage.on_pass, OnPass::Next);
}

#[test]
fn multi_stage_default_max_pipeline_iterations() {
    let dir = capsule_dir_with_config("stages:\n  - name: only\n    prompt: p.md\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(
        cfg.pipeline.max_pipeline_iterations,
        MAX_PIPELINE_ITERATIONS_DEFAULT
    );
}

#[test]
fn loop_block_parses_correctly() {
    let yaml = "\
stages:
  - loop:
      max_iteration: 10
      stages:
        - name: planner
          prompt: prompts/plan.md
        - name: doer
          prompt: prompts/do.md
          on_fail: planner
";
    let dir = capsule_dir_with_config(yaml);
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.pipeline.entries.len(), 1);
    let PipelineEntry::Loop(ref lp) = cfg.pipeline.entries[0] else {
        panic!("expected Loop entry");
    };
    assert_eq!(lp.max_iteration, Some(10));
    assert_eq!(lp.stages.len(), 2);
    assert_eq!(lp.stages[0].name, "planner");
    assert_eq!(lp.stages[1].name, "doer");
    assert_eq!(lp.stages[1].on_fail, OnFail::Stage("planner".to_string()));
}

#[test]
fn on_pass_exit_parses() {
    let yaml = "stages:\n  - name: only\n    on_pass: exit\n";
    let dir = capsule_dir_with_config(yaml);
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    let PipelineEntry::Stage(ref s) = cfg.pipeline.entries[0] else {
        panic!("expected Stage entry");
    };
    assert_eq!(s.on_pass, OnPass::Exit);
}

#[test]
fn on_fail_defaults_to_exit() {
    let yaml = "stages:\n  - name: only\n    prompt: p.md\n";
    let dir = capsule_dir_with_config(yaml);
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    let PipelineEntry::Stage(ref s) = cfg.pipeline.entries[0] else {
        panic!("expected Stage entry");
    };
    assert_eq!(s.on_fail, OnFail::Exit);
}

// ── Validation error tests ────────────────────────────────────────────────────

#[test]
fn iterations_combined_with_stages_is_rejected() {
    let yaml = "iterations: 5\nstages:\n  - name: foo\n";
    let dir = capsule_dir_with_config(yaml);
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("iterations") && chain.contains("stages"),
        "error should mention both fields; got: {chain}"
    );
}

#[test]
fn unknown_stage_reference_in_on_fail_is_rejected() {
    let yaml = "stages:\n  - name: foo\n    on_fail: nonexistent\n";
    let dir = capsule_dir_with_config(yaml);
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("nonexistent"),
        "error should name the unknown stage; got: {chain}"
    );
}

#[test]
fn unknown_stage_reference_in_on_pass_is_rejected() {
    let yaml = "stages:\n  - name: foo\n    on_pass: ghost\n";
    let dir = capsule_dir_with_config(yaml);
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("ghost"),
        "error should name the unknown stage; got: {chain}"
    );
}

#[test]
fn loop_stage_can_reference_another_loop_stage_in_on_fail() {
    let yaml = "\
stages:
  - loop:
      stages:
        - name: a
          on_fail: b
        - name: b
";
    let dir = capsule_dir_with_config(yaml);
    assert!(resolve(dir.path(), no_cli()).is_ok());
}

#[test]
fn multi_stage_model_and_verbose_parsed() {
    let yaml = "stages:\n  - name: s\nmodel: claude-haiku-4-5\nverbose: true\n";
    let dir = capsule_dir_with_config(yaml);
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("claude-haiku-4-5"));
    assert!(cfg.verbose);
}
