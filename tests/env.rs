use capsule::config::GithubScope;
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

#[test]
fn parse_dotenv_basic_key_value() {
    let env = parse_dotenv("FOO=bar\nBAZ=qux\n");
    assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("bar"));
    assert_eq!(env.get("BAZ").map(|s| s.as_str()), Some("qux"));
}

#[test]
fn parse_dotenv_empty_content_is_empty_map() {
    let env = parse_dotenv("");
    assert!(env.is_empty());
}

#[test]
fn parse_dotenv_ignores_comments_and_blank_lines() {
    let content = "# this is a comment\n\nFOO=hello\n\n# another comment\nBAR=world\n";
    let env = parse_dotenv(content);
    assert_eq!(env.len(), 2);
    assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("hello"));
    assert_eq!(env.get("BAR").map(|s| s.as_str()), Some("world"));
}

#[test]
fn parse_dotenv_strips_double_quotes() {
    let env = parse_dotenv("SECRET=\"my secret value\"\n");
    assert_eq!(
        env.get("SECRET").map(|s| s.as_str()),
        Some("my secret value")
    );
}

#[test]
fn parse_dotenv_strips_single_quotes() {
    let env = parse_dotenv("TOKEN='abc123'\n");
    assert_eq!(env.get("TOKEN").map(|s| s.as_str()), Some("abc123"));
}

#[test]
fn parse_dotenv_value_with_equals() {
    let env = parse_dotenv("URL=https://example.com/path?a=1&b=2\n");
    assert_eq!(
        env.get("URL").map(|s| s.as_str()),
        Some("https://example.com/path?a=1&b=2")
    );
}

#[test]
fn load_dotenv_absent_file_is_ok() {
    let dir = make_capsule_dir(None);
    assert!(load_dotenv(dir.path()).is_ok());
}

#[test]
fn resolve_gh_token_local_reads_from_dotenv_map() {
    let pre_env: HashMap<String, String> = HashMap::new();
    let mut dotenv: HashMap<String, String> = HashMap::new();
    dotenv.insert("GH_TOKEN".to_string(), "ghs_localtoken".to_string());
    let token = resolve_gh_token(&GithubScope::Local, &pre_env, &dotenv).unwrap();
    assert_eq!(token, "ghs_localtoken");
}

#[test]
fn resolve_gh_token_local_ignores_process_env() {
    let mut pre_env: HashMap<String, String> = HashMap::new();
    pre_env.insert("GH_TOKEN".to_string(), "ghs_processtoken".to_string());
    let mut dotenv: HashMap<String, String> = HashMap::new();
    dotenv.insert("GH_TOKEN".to_string(), "ghs_dotenvtoken".to_string());
    let token = resolve_gh_token(&GithubScope::Local, &pre_env, &dotenv).unwrap();
    // dotenv wins — process env is ignored for local scope
    assert_eq!(token, "ghs_dotenvtoken");
}

#[test]
fn resolve_gh_token_local_missing_returns_error() {
    let pre_env: HashMap<String, String> = HashMap::new();
    let dotenv: HashMap<String, String> = HashMap::new();
    let result = resolve_gh_token(&GithubScope::Local, &pre_env, &dotenv);
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("local"), "error should mention 'local': {msg}");
    assert!(
        msg.contains(".capsule/.env"),
        "error should name the file: {msg}"
    );
}

#[test]
fn resolve_gh_token_global_reads_from_pre_dotenv_env() {
    let mut pre_env: HashMap<String, String> = HashMap::new();
    pre_env.insert("GH_TOKEN".to_string(), "ghs_globaltoken".to_string());
    let dotenv: HashMap<String, String> = HashMap::new();
    let token = resolve_gh_token(&GithubScope::Global, &pre_env, &dotenv).unwrap();
    assert_eq!(token, "ghs_globaltoken");
}

#[test]
fn resolve_gh_token_global_missing_returns_error_or_token() {
    let pre_env: HashMap<String, String> = HashMap::new();
    let dotenv: HashMap<String, String> = HashMap::new();
    // Either returns a token from gh auth token, or returns an error.
    // We just assert it doesn't panic and that if it's an error the message is helpful.
    match resolve_gh_token(&GithubScope::Global, &pre_env, &dotenv) {
        Ok(_token) => { /* gh auth token succeeded in this environment — that's fine */ }
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                msg.contains("global"),
                "error should mention 'global': {msg}"
            );
        }
    }
}
