use capsule::prompt::{prepend_preamble, resolve_prompt, SYSTEM_PREAMBLE};
use std::path::PathBuf;
use tempfile::TempDir;

fn make_capsule_dir(prompt_content: Option<&str>) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    if let Some(content) = prompt_content {
        std::fs::write(dir.path().join("prompt.md"), content).unwrap();
    }
    dir
}

#[test]
fn default_prompt_md_is_read() {
    let dir = make_capsule_dir(Some("Hello, world!\n"));
    let contents = resolve_prompt(dir.path(), None).unwrap();
    assert_eq!(contents, b"Hello, world!\n");
}

#[test]
fn explicit_prompt_path_overrides_default() {
    let dir = make_capsule_dir(Some("default content"));
    let other = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(other.path(), "explicit content").unwrap();

    let contents = resolve_prompt(dir.path(), Some(other.path().to_path_buf())).unwrap();
    assert_eq!(contents, b"explicit content");
}

#[test]
fn missing_default_prompt_is_an_error_with_path() {
    let dir = make_capsule_dir(None);
    let err = resolve_prompt(dir.path(), None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("prompt.md"),
        "error should name the expected path; got: {msg}"
    );
}

#[test]
fn missing_explicit_prompt_is_an_error_with_path() {
    let dir = make_capsule_dir(None);
    let missing: PathBuf = dir.path().join("nonexistent.md");
    let err = resolve_prompt(dir.path(), Some(missing.clone())).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("nonexistent.md"),
        "error should name the missing path; got: {msg}"
    );
}

#[test]
fn prompt_contents_are_byte_for_byte() {
    let bytes: Vec<u8> = (0u8..=255u8).collect();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("prompt.md"), &bytes).unwrap();
    let contents = resolve_prompt(dir.path(), None).unwrap();
    assert_eq!(contents, bytes);
}

#[test]
fn system_preamble_is_non_empty() {
    assert!(!SYSTEM_PREAMBLE.trim().is_empty());
}

#[test]
fn preamble_is_prepended_before_user_content() {
    let result = prepend_preamble("do the thing");
    let preamble_pos = result.find(SYSTEM_PREAMBLE).unwrap();
    let user_pos = result.find("do the thing").unwrap();
    assert!(
        preamble_pos < user_pos,
        "preamble must come before user content"
    );
}
