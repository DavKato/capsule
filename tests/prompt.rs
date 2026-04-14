use capsule::prompt::resolve_prompt;
use std::path::PathBuf;
use tempfile::TempDir;

fn make_capsule_dir(prompt_content: Option<&str>) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    if let Some(content) = prompt_content {
        std::fs::write(dir.path().join("prompt.md"), content).unwrap();
    }
    dir
}

// ── Test 1 (tracer bullet): default prompt.md is read byte-for-byte ───────────
#[test]
fn default_prompt_md_is_read() {
    let dir = make_capsule_dir(Some("Hello, world!\n"));
    let contents = resolve_prompt(dir.path(), None).unwrap();
    assert_eq!(contents, b"Hello, world!\n");
}

// ── Test 2: explicit --prompt path overrides capsule_dir/prompt.md ────────────
#[test]
fn explicit_prompt_path_overrides_default() {
    let dir = make_capsule_dir(Some("default content"));
    let other = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(other.path(), "explicit content").unwrap();

    let contents = resolve_prompt(dir.path(), Some(other.path().to_path_buf())).unwrap();
    assert_eq!(contents, b"explicit content");
}

// ── Test 3: missing default prompt.md → error naming the expected path ─────────
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

// ── Test 4: missing explicit --prompt path → error naming that path ────────────
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

// ── Test 5: contents are returned byte-for-byte (binary safe) ─────────────────
#[test]
fn prompt_contents_are_byte_for_byte() {
    let bytes: Vec<u8> = (0u8..=255u8).collect();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("prompt.md"), &bytes).unwrap();
    let contents = resolve_prompt(dir.path(), None).unwrap();
    assert_eq!(contents, bytes);
}
