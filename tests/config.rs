use capsule::config::{
    resolve, CliOverrides, Config, GitIdentity, GithubScope, OnFail, OnPass, PipelineEntry,
    MAX_RETRIES_DEFAULT, MAX_STAGES_DEFAULT,
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

// ── Shared-field tests ───────────────────────────────────────────────────────

#[test]
fn no_config_file_shared_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert!(!cfg.rebuild);
    assert!(!cfg.verbose);
    assert_eq!(cfg.commit_as, GitIdentity::User);
}

#[test]
fn missing_config_file_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(resolve(dir.path(), no_cli()).is_ok());
}

#[test]
fn malformed_yaml_produces_clear_error() {
    let dir = capsule_dir_with_config(": this is not valid yaml: {\n");
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("config.yml"),
        "error message should name the file; got: {msg}"
    );
}

#[test]
fn config_file_model_and_verbose() {
    let dir =
        capsule_dir_with_config("stages:\n  - name: s\nmodel: claude-opus-4-6\nverbose: true\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("claude-opus-4-6"));
    assert!(cfg.verbose);
}

#[test]
fn commit_as_capsule_from_config_file() {
    let dir = capsule_dir_with_config("stages:\n  - name: s\ncommit_as: capsule\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.commit_as, GitIdentity::Capsule);
}

#[test]
fn github_token_from_absent_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert!(cfg.github_token_from.is_none());
}

#[test]
fn github_token_from_local_from_config_file() {
    let dir = capsule_dir_with_config("stages:\n  - name: s\ngithub_token_from: local\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.github_token_from, Some(GithubScope::Local));
}

#[test]
fn github_token_from_global_from_config_file() {
    let dir = capsule_dir_with_config("stages:\n  - name: s\ngithub_token_from: global\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.github_token_from, Some(GithubScope::Global));
}

#[test]
fn github_token_from_cli_overrides_config() {
    let dir = capsule_dir_with_config("stages:\n  - name: s\ngithub_token_from: global\n");
    let cli = CliOverrides {
        github_token_from: Some(GithubScope::Local),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert_eq!(cfg.github_token_from, Some(GithubScope::Local));
}

// ── Flat-form rejection tests ─────────────────────────────────────────────────

#[test]
fn flat_form_config_without_stages_is_rejected() {
    let dir = capsule_dir_with_config("iterations: 5\nprompt: prompts/implement.md\n");
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("stages"),
        "error should mention `stages:` key; got: {chain}"
    );
}

#[test]
fn flat_form_rejection_includes_migration_example() {
    let dir = capsule_dir_with_config("iterations: 3\n");
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("stages:") && chain.contains("max_stages"),
        "error should include before/after migration example; got: {chain}"
    );
}

#[test]
fn model_only_flat_form_is_rejected() {
    let dir = capsule_dir_with_config("model: claude-opus-4-6\n");
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("stages"),
        "error should mention `stages:` key; got: {chain}"
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
max_stages: 500
";

#[test]
fn multi_stage_parses_stages_and_routing() {
    let dir = capsule_dir_with_config(MULTI_STAGE_YAML);
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.pipeline.max_stages, 500);
    assert_eq!(cfg.pipeline.entries.len(), 2);

    let PipelineEntry::Stage(ref impl_stage) = cfg.pipeline.entries[0] else {
        panic!("expected Stage entry");
    };
    assert_eq!(impl_stage.name, "implementer");
    assert_eq!(impl_stage.on_fail, OnFail::Retry);
    assert_eq!(impl_stage.max_retries, 3);

    let PipelineEntry::Stage(ref rev_stage) = cfg.pipeline.entries[1] else {
        panic!("expected Stage entry");
    };
    assert_eq!(rev_stage.name, "reviewer");
    assert_eq!(rev_stage.on_fail, OnFail::Stage("implementer".to_string()));
    assert_eq!(rev_stage.on_pass, OnPass::Next);
}

#[test]
fn multi_stage_default_max_stages() {
    let dir = capsule_dir_with_config("stages:\n  - name: only\n    prompt: p.md\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.pipeline.max_stages, MAX_STAGES_DEFAULT);
}

#[test]
fn cli_max_stages_overrides_config_file() {
    let dir = capsule_dir_with_config(MULTI_STAGE_YAML); // config sets max_stages: 500
    let cli = CliOverrides {
        max_stages: Some(42),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert_eq!(cfg.pipeline.max_stages, 42);
}

#[test]
fn cli_max_stages_applies_without_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let cli = CliOverrides {
        max_stages: Some(7),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert_eq!(cfg.pipeline.max_stages, 7);
}

#[test]
fn multi_stage_max_retries_defaults_when_omitted_in_yaml() {
    let dir = capsule_dir_with_config(MULTI_STAGE_YAML);
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    let PipelineEntry::Stage(ref rev_stage) = cfg.pipeline.entries[1] else {
        panic!("expected Stage entry");
    };
    assert_eq!(rev_stage.name, "reviewer");
    assert_eq!(rev_stage.max_retries, MAX_RETRIES_DEFAULT);
}

#[test]
fn max_retries_defaults_to_constant_when_omitted() {
    let yaml = "\
stages:
  - name: worker
    prompt: prompts/work.md
    on_fail: retry
";
    let dir = capsule_dir_with_config(yaml);
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    let PipelineEntry::Stage(ref stage) = cfg.pipeline.entries[0] else {
        panic!("expected Stage entry");
    };
    assert_eq!(stage.max_retries, MAX_RETRIES_DEFAULT);
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
fn unknown_field_in_config_produces_clear_error() {
    let yaml = "stages:\n  - name: s\niteraions: 5\n";
    let dir = capsule_dir_with_config(yaml);
    let err = resolve(dir.path(), no_cli()).unwrap_err();
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
fn old_field_git_identity_produces_did_you_mean_error() {
    let dir = capsule_dir_with_config("git_identity: capsule\n");
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("git_identity") && chain.contains("commit_as"),
        "error should mention old and new field names; got: {chain}"
    );
}

#[test]
fn old_field_github_produces_did_you_mean_error() {
    let dir = capsule_dir_with_config("github: local\n");
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("github") && chain.contains("github_token_from"),
        "error should mention old and new field names; got: {chain}"
    );
}

#[test]
fn old_field_max_pipeline_iterations_produces_did_you_mean_error() {
    let yaml = "stages:\n  - name: foo\nmax_pipeline_iterations: 100\n";
    let dir = capsule_dir_with_config(yaml);
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("max_pipeline_iterations") && chain.contains("max_stages"),
        "error should mention old and new field names; got: {chain}"
    );
}

#[test]
fn old_field_min_token_lifetime_minutes_produces_removed_error() {
    let dir = capsule_dir_with_config("stages:\n  - name: s\nmin_token_lifetime_minutes: 30\n");
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("min_token_lifetime_minutes") && chain.contains("has been removed"),
        "error should mention field name and removal reason; got: {chain}"
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

#[test]
fn duplicate_stage_names_are_rejected() {
    let yaml = "\
stages:
  - name: foo
  - name: foo
";
    let dir = capsule_dir_with_config(yaml);
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("foo"),
        "error should name the duplicate stage; got: {chain}"
    );
}

#[test]
fn duplicate_name_across_top_level_and_loop_is_rejected() {
    let yaml = "\
stages:
  - name: foo
  - loop:
      stages:
        - name: foo
";
    let dir = capsule_dir_with_config(yaml);
    let err = resolve(dir.path(), no_cli()).unwrap_err();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        chain.contains("foo"),
        "error should name the duplicate stage; got: {chain}"
    );
}
