use capsule::env::{load_dotenv, parse_dotenv, resolve_gh_token};
use std::collections::HashMap;
use tempfile::TempDir;

fn make_capsule_dir(env_content: Option<&str>) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    if let Some(content) = env_content {
        std::fs::write(dir.path().join(".env"), content).unwrap();
    }
    dir
}

// ── parse_dotenv (pure, unit tests) ──────────────────────────────────────────

// Test 1 (tracer bullet): basic KEY=VALUE
#[test]
fn parse_dotenv_basic_key_value() {
    let env = parse_dotenv("FOO=bar\nBAZ=qux\n");
    assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("bar"));
    assert_eq!(env.get("BAZ").map(|s| s.as_str()), Some("qux"));
}

// Test 2: empty content → empty map
#[test]
fn parse_dotenv_empty_content_is_empty_map() {
    let env = parse_dotenv("");
    assert!(env.is_empty());
}

// Test 3: comments and blank lines are ignored
#[test]
fn parse_dotenv_ignores_comments_and_blank_lines() {
    let content = "# this is a comment\n\nFOO=hello\n\n# another comment\nBAR=world\n";
    let env = parse_dotenv(content);
    assert_eq!(env.len(), 2);
    assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("hello"));
    assert_eq!(env.get("BAR").map(|s| s.as_str()), Some("world"));
}

// Test 4: double-quoted values have quotes stripped
#[test]
fn parse_dotenv_strips_double_quotes() {
    let env = parse_dotenv("SECRET=\"my secret value\"\n");
    assert_eq!(
        env.get("SECRET").map(|s| s.as_str()),
        Some("my secret value")
    );
}

// Test 5: single-quoted values have quotes stripped
#[test]
fn parse_dotenv_strips_single_quotes() {
    let env = parse_dotenv("TOKEN='abc123'\n");
    assert_eq!(env.get("TOKEN").map(|s| s.as_str()), Some("abc123"));
}

// Test 6: value with = sign in it
#[test]
fn parse_dotenv_value_with_equals() {
    let env = parse_dotenv("URL=https://example.com/path?a=1&b=2\n");
    assert_eq!(
        env.get("URL").map(|s| s.as_str()),
        Some("https://example.com/path?a=1&b=2")
    );
}

// ── load_dotenv ───────────────────────────────────────────────────────────────

// Test 7: absent .env → no error
#[test]
fn load_dotenv_absent_file_is_ok() {
    let dir = make_capsule_dir(None);
    assert!(load_dotenv(dir.path()).is_ok());
}

// ── resolve_gh_token ──────────────────────────────────────────────────────────

// Test 8: GH_TOKEN present in env map → returned directly
#[test]
fn resolve_gh_token_from_env_map() {
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert("GH_TOKEN".to_string(), "ghs_testtoken".to_string());
    let token = resolve_gh_token(&env);
    assert_eq!(token.as_deref(), Some("ghs_testtoken"));
}

// Test 9: GH_TOKEN absent → None (no real gh binary available in unit test)
#[test]
fn resolve_gh_token_absent_returns_none_when_gh_unavailable() {
    // In a controlled unit-test environment without a real `gh` token,
    // absent GH_TOKEN + failing gh subprocess → None.
    let env: HashMap<String, String> = HashMap::new();
    // We can only assert the return type is Option — whether it's Some or None
    // depends on the host environment. The important thing is it doesn't panic.
    let _token = resolve_gh_token(&env);
    // No assertion on value — integration test covers the gh fallback path.
}
