use crate::verdict::{Verdict, VerdictStatus};
use serde_json::Value;

/// Scans Claude Code stream-json lines for `submit_verdict` tool_use events.
/// Last-wins: calling `feed` multiple times keeps the latest valid verdict.
pub struct StreamParser {
    verdict: Option<Verdict>,
    auth_failed: bool,
}

impl StreamParser {
    pub fn new() -> Self {
        Self {
            verdict: None,
            auth_failed: false,
        }
    }

    /// Feed one line of stream-json. Returns the latest valid verdict seen so
    /// far (updated when this line contains a valid `submit_verdict` call).
    pub fn feed(&mut self, line: &str) -> Option<&Verdict> {
        if line.contains("authentication_failed") {
            self.auth_failed = true;
        }
        if let Some(v) = extract_verdict(line) {
            self.verdict = Some(v);
        }
        self.verdict.as_ref()
    }

    pub fn verdict(&self) -> Option<&Verdict> {
        self.verdict.as_ref()
    }

    pub fn auth_failed(&self) -> bool {
        self.auth_failed
    }
}

impl Default for StreamParser {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_verdict(line: &str) -> Option<Verdict> {
    let msg: Value = serde_json::from_str(line).ok()?;
    if msg.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let content = msg.pointer("/message/content")?.as_array()?;
    for block in content {
        if block.get("type")?.as_str()? == "tool_use"
            && block.get("name")?.as_str()? == "submit_verdict"
        {
            let input = block.get("input")?;
            let status_str = input.get("status")?.as_str()?;
            let status = match status_str {
                "pass" => VerdictStatus::Pass,
                "fail" => VerdictStatus::Fail,
                _ => continue,
            };
            let notes = input
                .get("notes")
                .and_then(Value::as_str)
                .map(str::to_owned);
            return Some(Verdict { status, notes });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::VerdictStatus;

    const PASS_LINE: &str = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_01abc","name":"submit_verdict","input":{"status":"pass","notes":"all done"}}]}}"#;
    const FAIL_LINE: &str = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_02def","name":"submit_verdict","input":{"status":"fail","notes":"tests broke"}}]}}"#;
    const TEXT_LINE: &str =
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"thinking..."}]}}"#;
    const RESULT_LINE: &str = r#"{"type":"result","subtype":"success","result":"done"}"#;
    const AUTH_FAIL_LINE: &str = r#"{"type":"result","subtype":"error","error":{"type":"authentication_failed","message":"invalid token"}}"#;

    #[test]
    fn non_json_returns_none() {
        let mut p = StreamParser::new();
        assert!(p.feed("not json at all").is_none());
    }

    #[test]
    fn non_assistant_event_returns_none() {
        let mut p = StreamParser::new();
        assert!(p.feed(RESULT_LINE).is_none());
    }

    #[test]
    fn text_content_returns_none() {
        let mut p = StreamParser::new();
        assert!(p.feed(TEXT_LINE).is_none());
    }

    #[test]
    fn pass_line_returns_pass_verdict() {
        let mut p = StreamParser::new();
        let v = p.feed(PASS_LINE).unwrap();
        assert_eq!(v.status, VerdictStatus::Pass);
        assert_eq!(v.notes.as_deref(), Some("all done"));
    }

    #[test]
    fn fail_line_returns_fail_verdict() {
        let mut p = StreamParser::new();
        let v = p.feed(FAIL_LINE).unwrap();
        assert_eq!(v.status, VerdictStatus::Fail);
    }

    #[test]
    fn fail_verdict_preserves_notes() {
        let mut p = StreamParser::new();
        let v = p.feed(FAIL_LINE).unwrap();
        assert_eq!(v.status, VerdictStatus::Fail);
        assert_eq!(v.notes.as_deref(), Some("tests broke"));
    }

    #[test]
    fn last_wins_on_duplicate_calls() {
        let mut p = StreamParser::new();
        p.feed(PASS_LINE);
        let v = p.feed(FAIL_LINE).unwrap();
        assert_eq!(v.status, VerdictStatus::Fail);
    }

    #[test]
    fn fail_then_pass_last_wins_is_pass() {
        let mut p = StreamParser::new();
        p.feed(FAIL_LINE);
        let v = p.feed(PASS_LINE).unwrap();
        assert_eq!(v.status, VerdictStatus::Pass);
    }

    #[test]
    fn no_verdict_in_stream_returns_none() {
        let mut p = StreamParser::new();
        p.feed(TEXT_LINE);
        p.feed(RESULT_LINE);
        p.feed("not json");
        assert!(p.verdict().is_none());
    }

    #[test]
    fn verdict_without_notes_is_valid() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_03","name":"submit_verdict","input":{"status":"pass"}}]}}"#;
        let mut p = StreamParser::new();
        let v = p.feed(line).unwrap();
        assert_eq!(v.status, VerdictStatus::Pass);
        assert!(v.notes.is_none());
    }

    #[test]
    fn invalid_status_enum_is_skipped() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_04","name":"submit_verdict","input":{"status":"done"}}]}}"#;
        let mut p = StreamParser::new();
        assert!(p.feed(line).is_none());
    }

    #[test]
    fn non_verdict_tool_use_is_skipped() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_05","name":"Bash","input":{"command":"ls"}}]}}"#;
        let mut p = StreamParser::new();
        assert!(p.feed(line).is_none());
    }

    #[test]
    fn verdict_persists_across_non_verdict_lines() {
        let mut p = StreamParser::new();
        p.feed(PASS_LINE);
        let v = p.feed(TEXT_LINE).unwrap();
        assert_eq!(v.status, VerdictStatus::Pass);
    }

    #[test]
    fn auth_failure_line_sets_auth_failed() {
        let mut p = StreamParser::new();
        p.feed(AUTH_FAIL_LINE);
        assert!(p.auth_failed());
    }

    #[test]
    fn normal_line_does_not_set_auth_failed() {
        let mut p = StreamParser::new();
        p.feed(TEXT_LINE);
        assert!(!p.auth_failed());
    }
}
