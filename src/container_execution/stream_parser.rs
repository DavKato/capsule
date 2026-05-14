use crate::verdict::{Verdict, VerdictStatus};
use serde_json::Value;

pub struct ToolUseEvent {
    pub id: String,
    pub name: String,
    pub input: Value,
}

pub struct ToolResultEvent {
    pub tool_use_id: String,
    pub is_error: bool,
    pub content: Option<String>,
}

pub enum ToolEvent {
    Use(ToolUseEvent),
    Result(ToolResultEvent),
}

pub enum TextDisplay {
    Content(String),
    Thinking(String),
}

pub struct UsageSnapshot {
    pub input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct ModelUsage {
    pub context_window: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Format total token usage with context window percentage, e.g. `"33.3k (16.6%) used"`.
pub fn format_usage_with_percentage(total_tokens: u64, context_window: u64) -> String {
    let k = total_tokens as f64 / 1000.0;
    let pct = if context_window > 0 {
        total_tokens as f64 / context_window as f64 * 100.0
    } else {
        0.0
    };
    format!("{k:.1}k ({pct:.1}%) used")
}

impl UsageSnapshot {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
            + self.output_tokens
    }
}

pub struct StreamParser {
    verdict: Option<Verdict>,
    auth_failed: bool,
    init_seen: bool,
    submit_verdict_registered: bool,
    session_id: Option<String>,
    last_tool_events: Vec<ToolEvent>,
    last_text_displays: Vec<TextDisplay>,
    last_parsed_value: Option<Value>,
    last_usage_snapshot: Option<UsageSnapshot>,
    model_usage: Option<ModelUsage>,
}

impl StreamParser {
    pub fn new() -> Self {
        Self {
            verdict: None,
            auth_failed: false,
            init_seen: false,
            submit_verdict_registered: false,
            session_id: None,
            last_tool_events: Vec::new(),
            last_text_displays: Vec::new(),
            last_parsed_value: None,
            last_usage_snapshot: None,
            model_usage: None,
        }
    }

    pub fn feed(&mut self, line: &str) -> Option<&Verdict> {
        self.last_tool_events.clear();
        self.last_text_displays.clear();
        self.last_parsed_value = None;
        self.last_usage_snapshot = None;
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return self.verdict.as_ref();
        };
        if is_auth_error(&msg) {
            self.auth_failed = true;
        }
        if is_init_event(&msg) {
            self.init_seen = true;
            self.session_id = extract_session_id(&msg);
            if let Some(tools) = extract_init_tools(&msg) {
                self.submit_verdict_registered = tools.iter().any(|t| {
                    let name = t.as_str().or_else(|| t.get("name").and_then(Value::as_str));
                    name.is_some_and(is_submit_verdict)
                });
            }
        }
        if let Some(v) = extract_verdict(&msg) {
            if self.verdict.is_some() {
                crate::display::warning("submit_verdict called more than once; using latest");
            }
            self.verdict = Some(v);
        }
        let (tool_events, text_displays) = extract_assistant_content(&msg);
        self.last_tool_events = tool_events;
        self.last_text_displays = text_displays;
        self.last_usage_snapshot = extract_usage_snapshot(&msg);
        if let Some(usage) = extract_model_usage(&msg) {
            self.model_usage = Some(usage);
        }
        if msg.get("type").is_some() {
            self.last_parsed_value = Some(msg);
        }
        self.verdict.as_ref()
    }

    pub fn verdict(&self) -> Option<&Verdict> {
        self.verdict.as_ref()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn auth_failed(&self) -> bool {
        self.auth_failed
    }

    /// True when the init event was seen but `submit_verdict` was not in the tool list.
    pub fn submit_verdict_missing(&self) -> bool {
        self.init_seen && !self.submit_verdict_registered
    }

    /// Returns all tool events extracted from the most recent `feed()` call.
    pub fn last_tool_events(&self) -> &[ToolEvent] {
        &self.last_tool_events
    }

    /// Returns text/thinking display items extracted from the most recent `feed()` call.
    pub fn last_text_displays(&self) -> &[TextDisplay] {
        &self.last_text_displays
    }

    /// Returns the parsed JSON value from the most recent `feed()` call, if the line
    /// was valid JSON containing a `"type"` field.
    pub fn last_parsed_value(&self) -> Option<&Value> {
        self.last_parsed_value.as_ref()
    }

    /// Returns the usage snapshot extracted from the most recent `feed()` call, if the
    /// line was an assistant message carrying `message.usage`.
    pub fn last_usage_snapshot(&self) -> Option<&UsageSnapshot> {
        self.last_usage_snapshot.as_ref()
    }

    /// Returns model usage extracted from the final `"type": "result"` message, if seen.
    pub fn model_usage(&self) -> Option<&ModelUsage> {
        self.model_usage.as_ref()
    }
}

impl Default for StreamParser {
    fn default() -> Self {
        Self::new()
    }
}

fn is_auth_error(msg: &Value) -> bool {
    msg.pointer("/error/type").and_then(Value::as_str) == Some("authentication_failed")
}

fn is_init_event(msg: &Value) -> bool {
    msg.get("type").and_then(Value::as_str) == Some("system")
        && msg.get("subtype").and_then(Value::as_str) == Some("init")
}

fn extract_session_id(msg: &Value) -> Option<String> {
    let id = msg.get("session_id")?.as_str()?;
    is_valid_session_id(id).then(|| id.to_owned())
}

fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

fn is_submit_verdict(name: &str) -> bool {
    name == "submit_verdict" || name.ends_with("__submit_verdict")
}

fn extract_init_tools(msg: &Value) -> Option<Vec<Value>> {
    Some(msg.get("tools")?.as_array()?.clone())
}

fn extract_assistant_content(msg: &Value) -> (Vec<ToolEvent>, Vec<TextDisplay>) {
    let Some(msg_type) = msg.get("type").and_then(Value::as_str) else {
        return (Vec::new(), Vec::new());
    };
    match msg_type {
        "assistant" => {
            let Some(content) = msg.pointer("/message/content").and_then(Value::as_array) else {
                return (Vec::new(), Vec::new());
            };
            let mut tool_events = Vec::new();
            let mut text_displays = Vec::new();
            for block in content {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_use") => {
                        if let (Some(name), Some(id), Some(input)) = (
                            block.get("name").and_then(Value::as_str).map(str::to_owned),
                            block.get("id").and_then(Value::as_str).map(str::to_owned),
                            block.get("input").cloned(),
                        ) {
                            tool_events.push(ToolEvent::Use(ToolUseEvent { id, name, input }));
                        }
                    }
                    Some("text") => {
                        if let Some(text) =
                            block.get("text").and_then(Value::as_str).map(str::to_owned)
                        {
                            text_displays.push(TextDisplay::Content(text));
                        }
                    }
                    Some("thinking") => {
                        if let Some(text) = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                        {
                            text_displays.push(TextDisplay::Thinking(text));
                        }
                    }
                    _ => {}
                }
            }
            (tool_events, text_displays)
        }
        "user" => {
            let Some(content) = msg.pointer("/message/content").and_then(Value::as_array) else {
                return (Vec::new(), Vec::new());
            };
            let tool_events = content
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
                .filter_map(|block| {
                    let tool_use_id = block.get("tool_use_id")?.as_str()?.to_owned();
                    let is_error = block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let content = block
                        .get("content")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    Some(ToolEvent::Result(ToolResultEvent {
                        tool_use_id,
                        is_error,
                        content,
                    }))
                })
                .collect();
            (tool_events, Vec::new())
        }
        _ => (Vec::new(), Vec::new()),
    }
}

fn extract_usage_snapshot(msg: &Value) -> Option<UsageSnapshot> {
    if msg.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let usage = msg.pointer("/message/usage")?;
    let get_u64 = |key: &str| -> u64 { usage.get(key).and_then(Value::as_u64).unwrap_or(0) };
    Some(UsageSnapshot {
        input_tokens: get_u64("input_tokens"),
        cache_creation_input_tokens: get_u64("cache_creation_input_tokens"),
        cache_read_input_tokens: get_u64("cache_read_input_tokens"),
        output_tokens: get_u64("output_tokens"),
    })
}

fn extract_model_usage(msg: &Value) -> Option<ModelUsage> {
    if msg.get("type").and_then(Value::as_str) != Some("result") {
        return None;
    }
    let model_usage_map = msg.get("modelUsage")?.as_object()?;
    model_usage_map
        .values()
        .filter_map(|v| {
            let context_window = v.get("contextWindow")?.as_u64()?;
            let input_tokens = v.get("inputTokens").and_then(Value::as_u64).unwrap_or(0);
            let output_tokens = v.get("outputTokens").and_then(Value::as_u64).unwrap_or(0);
            let cost_usd = v.get("costUSD").and_then(Value::as_f64).unwrap_or(0.0);
            Some(ModelUsage {
                context_window,
                input_tokens,
                output_tokens,
                cost_usd,
            })
        })
        .max_by_key(|u| u.context_window)
}

fn extract_verdict(msg: &Value) -> Option<Verdict> {
    if msg.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let content = msg.pointer("/message/content")?.as_array()?;
    for block in content {
        if block.get("type")?.as_str()? == "tool_use"
            && block.get("name")?.as_str().is_some_and(is_submit_verdict)
        {
            let input = block.get("input")?;
            let status_str = input.get("status")?.as_str()?;
            let status = match status_str {
                "pass" => VerdictStatus::Pass,
                "fail" => VerdictStatus::Fail,
                "done" => VerdictStatus::Done,
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
    fn last_parsed_value_some_for_typed_json() {
        let mut p = StreamParser::new();
        p.feed(r#"{"type":"system.init","foo":"bar"}"#);
        assert!(p.last_parsed_value().is_some());
    }

    #[test]
    fn last_parsed_value_none_for_json_without_type() {
        let mut p = StreamParser::new();
        p.feed(r#"{"foo":"bar"}"#);
        assert!(p.last_parsed_value().is_none());
    }

    #[test]
    fn last_parsed_value_none_for_non_json() {
        let mut p = StreamParser::new();
        p.feed("plain text line");
        assert!(p.last_parsed_value().is_none());
        p.feed("GH_TOKEN: local (.capsule/.env)");
        assert!(p.last_parsed_value().is_none());
        p.feed("");
        assert!(p.last_parsed_value().is_none());
    }

    #[test]
    fn last_parsed_value_none_for_rate_limit_adjacent_non_json() {
        let mut p = StreamParser::new();
        p.feed("working on: #126");
        assert!(p.last_parsed_value().is_none());
    }

    #[test]
    fn last_parsed_value_some_for_different_type_values() {
        let mut p = StreamParser::new();
        for ty in &["rate_limit_event", "user", "assistant", "system.init"] {
            let line = format!(r#"{{"type":"{ty}"}}"#);
            p.feed(&line);
            assert!(
                p.last_parsed_value().is_some(),
                "type={ty} must yield last_parsed_value"
            );
        }
    }

    #[test]
    fn last_parsed_value_cleared_between_feeds() {
        let mut p = StreamParser::new();
        p.feed(r#"{"type":"assistant","foo":"bar"}"#);
        assert!(p.last_parsed_value().is_some());
        p.feed("plain text");
        assert!(p.last_parsed_value().is_none());
    }

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
    fn done_status_returns_done_verdict() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_04","name":"submit_verdict","input":{"status":"done","notes":"scope complete"}}]}}"#;
        let mut p = StreamParser::new();
        let v = p.feed(line).unwrap();
        assert_eq!(v.status, VerdictStatus::Done);
        assert_eq!(v.notes.as_deref(), Some("scope complete"));
    }

    #[test]
    fn unknown_status_enum_is_skipped() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_05","name":"submit_verdict","input":{"status":"unknown"}}]}}"#;
        let mut p = StreamParser::new();
        assert!(p.feed(line).is_none());
    }

    #[test]
    fn mcp_prefixed_verdict_is_extracted() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_06","name":"mcp__capsule__submit_verdict","input":{"status":"pass","notes":"done"}}]}}"#;
        let mut p = StreamParser::new();
        let v = p.feed(line).unwrap();
        assert_eq!(v.status, VerdictStatus::Pass);
        assert_eq!(v.notes.as_deref(), Some("done"));
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

    // Claude Code stream-json: tools are plain strings, MCP tools prefixed mcp__<server>__<tool>.
    const SYSTEM_INIT_WITH_VERDICT_TOOL: &str = r#"{"type":"system","subtype":"init","session_id":"sess_01","tools":["Bash","Read","mcp__capsule__submit_verdict"]}"#;
    const SYSTEM_INIT_BARE_VERDICT_TOOL: &str =
        r#"{"type":"system","subtype":"init","session_id":"sess_03","tools":["submit_verdict"]}"#;
    const SYSTEM_INIT_WITHOUT_VERDICT_TOOL: &str = r#"{"type":"system","subtype":"init","session_id":"sess_02","tools":["Bash","Read","Write"]}"#;

    #[test]
    fn system_init_with_submit_verdict_marks_registered() {
        let mut p = StreamParser::new();
        p.feed(SYSTEM_INIT_WITH_VERDICT_TOOL);
        assert!(!p.submit_verdict_missing());
    }

    #[test]
    fn system_init_with_bare_submit_verdict_marks_registered() {
        let mut p = StreamParser::new();
        p.feed(SYSTEM_INIT_BARE_VERDICT_TOOL);
        assert!(!p.submit_verdict_missing());
    }

    #[test]
    fn system_init_without_submit_verdict_signals_missing() {
        let mut p = StreamParser::new();
        p.feed(SYSTEM_INIT_WITHOUT_VERDICT_TOOL);
        assert!(p.submit_verdict_missing());
    }

    #[test]
    fn no_init_event_does_not_signal_missing() {
        let mut p = StreamParser::new();
        p.feed(TEXT_LINE);
        p.feed(PASS_LINE);
        assert!(!p.submit_verdict_missing());
    }

    #[test]
    fn system_init_with_null_tools_still_signals_missing() {
        let line = r#"{"type":"system","subtype":"init","session_id":"sess_04","tools":null}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        assert!(p.submit_verdict_missing());
    }

    #[test]
    fn system_init_with_missing_tools_field_still_signals_missing() {
        let line = r#"{"type":"system","subtype":"init","session_id":"sess_05"}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        assert!(p.submit_verdict_missing());
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

    #[test]
    fn session_id_captured_from_init_event() {
        let mut p = StreamParser::new();
        p.feed(SYSTEM_INIT_WITH_VERDICT_TOOL);
        assert_eq!(p.session_id(), Some("sess_01"));
    }

    #[test]
    fn session_id_none_before_init() {
        let p = StreamParser::new();
        assert_eq!(p.session_id(), None);
    }

    #[test]
    fn session_id_none_when_init_lacks_field() {
        let line = r#"{"type":"system","subtype":"init","tools":["Bash"]}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        assert_eq!(p.session_id(), None);
    }

    #[test]
    fn session_id_accepted_without_sess_prefix() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc123","tools":["Bash"]}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        assert_eq!(p.session_id(), Some("abc123"));
    }

    #[test]
    fn session_id_accepted_uuid_format() {
        let line = r#"{"type":"system","subtype":"init","session_id":"01f202b4-9b8e-44f8-a811-f1d0b75094db","tools":["Bash"]}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        assert_eq!(p.session_id(), Some("01f202b4-9b8e-44f8-a811-f1d0b75094db"));
    }

    #[test]
    fn session_id_rejected_with_special_chars() {
        let line =
            r#"{"type":"system","subtype":"init","session_id":"sess_foo$bar","tools":["Bash"]}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        assert_eq!(p.session_id(), None);
    }

    #[test]
    fn session_id_rejected_if_empty() {
        let line = r#"{"type":"system","subtype":"init","session_id":"","tools":["Bash"]}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        assert_eq!(p.session_id(), None);
    }

    #[test]
    fn session_id_accepted_with_hyphens_and_underscores() {
        let line = r#"{"type":"system","subtype":"init","session_id":"sess_foo-bar_baz","tools":["Bash"]}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        assert_eq!(p.session_id(), Some("sess_foo-bar_baz"));
    }

    const TOOL_USE_LINE: &str = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_bash01","name":"Bash","input":{"command":"ls -la"}}]}}"#;
    const TOOL_RESULT_LINE: &str = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_bash01","content":"file1.txt\nfile2.txt","is_error":false}]}}"#;
    const TOOL_RESULT_ERROR_LINE: &str = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_bash01","content":"command not found","is_error":true}]}}"#;
    const MCP_TOOL_USE_LINE: &str = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_mcp01","name":"mcp__capsule__run_task","input":{"task":"build"}}]}}"#;

    #[test]
    fn tool_use_event_is_extracted() {
        let mut p = StreamParser::new();
        p.feed(TOOL_USE_LINE);
        let events = p.last_tool_events();
        assert_eq!(events.len(), 1);
        let ToolEvent::Use(use_event) = &events[0] else {
            panic!("expected ToolEvent::Use");
        };
        assert_eq!(use_event.id, "toolu_bash01");
        assert_eq!(use_event.name, "Bash");
        assert_eq!(use_event.input["command"], "ls -la");
    }

    #[test]
    fn tool_result_success_event_is_extracted() {
        let mut p = StreamParser::new();
        p.feed(TOOL_RESULT_LINE);
        let events = p.last_tool_events();
        assert_eq!(events.len(), 1);
        let ToolEvent::Result(result_event) = &events[0] else {
            panic!("expected ToolEvent::Result");
        };
        assert_eq!(result_event.tool_use_id, "toolu_bash01");
        assert!(!result_event.is_error);
        assert_eq!(
            result_event.content.as_deref(),
            Some("file1.txt\nfile2.txt")
        );
    }

    #[test]
    fn tool_result_error_event_is_extracted() {
        let mut p = StreamParser::new();
        p.feed(TOOL_RESULT_ERROR_LINE);
        let events = p.last_tool_events();
        assert_eq!(events.len(), 1);
        let ToolEvent::Result(result_event) = &events[0] else {
            panic!("expected ToolEvent::Result");
        };
        assert_eq!(result_event.tool_use_id, "toolu_bash01");
        assert!(result_event.is_error);
        assert_eq!(result_event.content.as_deref(), Some("command not found"));
    }

    #[test]
    fn tool_result_with_no_content_field_yields_none() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_bash01","is_error":false}]}}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        let events = p.last_tool_events();
        assert_eq!(events.len(), 1);
        let ToolEvent::Result(result_event) = &events[0] else {
            panic!("expected ToolEvent::Result");
        };
        assert_eq!(result_event.tool_use_id, "toolu_bash01");
        assert!(result_event.content.is_none());
    }

    #[test]
    fn interleaved_tool_use_and_result_events() {
        let mut p = StreamParser::new();

        p.feed(TOOL_USE_LINE);
        assert_eq!(p.last_tool_events().len(), 1);
        assert!(matches!(&p.last_tool_events()[0], ToolEvent::Use(_)));

        p.feed(TOOL_RESULT_LINE);
        assert_eq!(p.last_tool_events().len(), 1);
        let ToolEvent::Result(result_event) = &p.last_tool_events()[0] else {
            panic!("expected ToolEvent::Result");
        };
        assert_eq!(result_event.tool_use_id, "toolu_bash01");
    }

    #[test]
    fn mcp_prefixed_tool_use_is_extracted() {
        let mut p = StreamParser::new();
        p.feed(MCP_TOOL_USE_LINE);
        let events = p.last_tool_events();
        assert_eq!(events.len(), 1);
        let ToolEvent::Use(use_event) = &events[0] else {
            panic!("expected ToolEvent::Use");
        };
        assert_eq!(use_event.name, "mcp__capsule__run_task");
        assert_eq!(use_event.id, "toolu_mcp01");
        assert_eq!(use_event.input["task"], "build");
    }

    #[test]
    fn non_tool_line_clears_last_tool_events() {
        let mut p = StreamParser::new();
        p.feed(TOOL_USE_LINE);
        assert!(!p.last_tool_events().is_empty());
        p.feed(TEXT_LINE);
        assert!(p.last_tool_events().is_empty());
    }

    #[test]
    fn no_tool_events_before_any_feed() {
        let p = StreamParser::new();
        assert!(p.last_tool_events().is_empty());
    }

    #[test]
    fn submit_verdict_tool_use_also_emits_tool_event() {
        let mut p = StreamParser::new();
        p.feed(PASS_LINE);
        let events = p.last_tool_events();
        assert_eq!(events.len(), 1);
        let ToolEvent::Use(use_event) = &events[0] else {
            panic!("expected ToolEvent::Use");
        };
        assert_eq!(use_event.name, "submit_verdict");
    }

    #[test]
    fn parallel_tool_calls_extracted() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_01","name":"Bash","input":{"command":"ls"}},{"type":"tool_use","id":"toolu_02","name":"Read","input":{"path":"/tmp/foo"}}]}}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        let events = p.last_tool_events();
        assert_eq!(events.len(), 2);
        let ToolEvent::Use(first) = &events[0] else {
            panic!("expected ToolEvent::Use");
        };
        assert_eq!(first.name, "Bash");
        assert_eq!(first.id, "toolu_01");
        let ToolEvent::Use(second) = &events[1] else {
            panic!("expected ToolEvent::Use");
        };
        assert_eq!(second.name, "Read");
        assert_eq!(second.id, "toolu_02");
    }

    #[test]
    fn multiple_tool_results_extracted() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_01","is_error":false},{"type":"tool_result","tool_use_id":"toolu_02","is_error":true}]}}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        let events = p.last_tool_events();
        assert_eq!(events.len(), 2);
        let ToolEvent::Result(first) = &events[0] else {
            panic!("expected ToolEvent::Result");
        };
        assert_eq!(first.tool_use_id, "toolu_01");
        assert!(!first.is_error);
        let ToolEvent::Result(second) = &events[1] else {
            panic!("expected ToolEvent::Result");
        };
        assert_eq!(second.tool_use_id, "toolu_02");
        assert!(second.is_error);
    }

    #[test]
    fn text_content_extracted_from_assistant_message() {
        let mut p = StreamParser::new();
        p.feed(TEXT_LINE);
        let displays = p.last_text_displays();
        assert_eq!(displays.len(), 1);
        assert!(matches!(&displays[0], TextDisplay::Content(t) if t == "thinking..."));
    }

    #[test]
    fn thinking_content_extracted_from_assistant_message() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm let me think"}]}}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        let displays = p.last_text_displays();
        assert_eq!(displays.len(), 1);
        assert!(matches!(&displays[0], TextDisplay::Thinking(t) if t == "hmm let me think"));
    }

    #[test]
    fn mixed_thinking_and_text_blocks_both_extracted() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm let me think"},{"type":"text","text":"here is my answer"}]}}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        let displays = p.last_text_displays();
        assert_eq!(displays.len(), 2);
        assert!(matches!(&displays[0], TextDisplay::Thinking(t) if t == "hmm let me think"));
        assert!(matches!(&displays[1], TextDisplay::Content(t) if t == "here is my answer"));
    }

    #[test]
    fn mixed_text_and_tool_use_both_extracted() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"I'll run this"},{"type":"tool_use","id":"toolu_01","name":"Bash","input":{"command":"ls"}}]}}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        let tool_events = p.last_tool_events();
        assert_eq!(tool_events.len(), 1);
        let ToolEvent::Use(tu) = &tool_events[0] else {
            panic!("expected ToolEvent::Use");
        };
        assert_eq!(tu.name, "Bash");
        let text_displays = p.last_text_displays();
        assert_eq!(text_displays.len(), 1);
        assert!(matches!(&text_displays[0], TextDisplay::Content(t) if t == "I'll run this"));
    }

    #[test]
    fn user_message_has_no_text_displays() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_01","is_error":false}]}}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        assert!(p.last_text_displays().is_empty());
    }

    #[test]
    fn tool_only_message_has_no_text_displays() {
        let mut p = StreamParser::new();
        p.feed(TOOL_USE_LINE);
        assert!(p.last_text_displays().is_empty());
    }

    #[test]
    fn text_displays_cleared_between_feeds() {
        let mut p = StreamParser::new();
        p.feed(TEXT_LINE);
        assert!(!p.last_text_displays().is_empty());
        p.feed(TOOL_USE_LINE);
        assert!(p.last_text_displays().is_empty());
    }

    #[test]
    fn no_text_displays_before_any_feed() {
        let p = StreamParser::new();
        assert!(p.last_text_displays().is_empty());
    }

    const ASSISTANT_WITH_USAGE: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":9,"cache_creation_input_tokens":32784,"cache_read_input_tokens":100,"output_tokens":8,"service_tier":"standard"}}}"#;
    const ASSISTANT_WITHOUT_USAGE: &str =
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;

    #[test]
    fn assistant_with_usage_yields_snapshot() {
        let mut p = StreamParser::new();
        p.feed(ASSISTANT_WITH_USAGE);
        let snap = p.last_usage_snapshot().expect("expected usage snapshot");
        assert_eq!(snap.input_tokens, 9);
        assert_eq!(snap.cache_creation_input_tokens, 32784);
        assert_eq!(snap.cache_read_input_tokens, 100);
        assert_eq!(snap.output_tokens, 8);
    }

    #[test]
    fn usage_snapshot_total_tokens_sums_all_fields() {
        let mut p = StreamParser::new();
        p.feed(ASSISTANT_WITH_USAGE);
        let snap = p.last_usage_snapshot().expect("expected usage snapshot");
        assert_eq!(snap.total_tokens(), 9 + 32784 + 100 + 8);
    }

    #[test]
    fn assistant_without_usage_yields_no_snapshot() {
        let mut p = StreamParser::new();
        p.feed(ASSISTANT_WITHOUT_USAGE);
        assert!(p.last_usage_snapshot().is_none());
    }

    #[test]
    fn non_assistant_message_yields_no_snapshot() {
        let mut p = StreamParser::new();
        p.feed(RESULT_LINE);
        assert!(p.last_usage_snapshot().is_none());
    }

    #[test]
    fn missing_optional_cache_fields_default_to_zero() {
        let line = r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":5,"output_tokens":3}}}"#;
        let mut p = StreamParser::new();
        p.feed(line);
        let snap = p.last_usage_snapshot().expect("expected usage snapshot");
        assert_eq!(snap.input_tokens, 5);
        assert_eq!(snap.cache_creation_input_tokens, 0);
        assert_eq!(snap.cache_read_input_tokens, 0);
        assert_eq!(snap.output_tokens, 3);
        assert_eq!(snap.total_tokens(), 8);
    }

    #[test]
    fn usage_snapshot_cleared_between_feeds() {
        let mut p = StreamParser::new();
        p.feed(ASSISTANT_WITH_USAGE);
        assert!(p.last_usage_snapshot().is_some());
        p.feed(ASSISTANT_WITHOUT_USAGE);
        assert!(p.last_usage_snapshot().is_none());
    }

    const RESULT_WITH_MODEL_USAGE: &str = r#"{"type":"result","subtype":"success","result":"done","modelUsage":{"claude-haiku-4-5-20251001":{"contextWindow":200000,"maxOutputTokens":32000,"inputTokens":388,"outputTokens":163,"costUSD":0.017286}}}"#;
    const RESULT_MULTI_MODEL: &str = r#"{"type":"result","subtype":"success","result":"done","modelUsage":{"claude-haiku-4-5-20251001":{"contextWindow":200000,"inputTokens":100,"outputTokens":50,"costUSD":0.01},"claude-opus-4-7":{"contextWindow":1000000,"inputTokens":200,"outputTokens":80,"costUSD":0.05}}}"#;

    #[test]
    fn result_with_model_usage_extracts_correctly() {
        let mut p = StreamParser::new();
        p.feed(RESULT_WITH_MODEL_USAGE);
        let usage = p.model_usage().expect("expected model usage");
        assert_eq!(usage.context_window, 200000);
        assert_eq!(usage.input_tokens, 388);
        assert_eq!(usage.output_tokens, 163);
        assert!((usage.cost_usd - 0.017286).abs() < 1e-6);
    }

    #[test]
    fn result_without_model_usage_yields_none() {
        let mut p = StreamParser::new();
        p.feed(RESULT_LINE);
        assert!(p.model_usage().is_none());
    }

    #[test]
    fn non_result_message_does_not_set_model_usage() {
        let mut p = StreamParser::new();
        p.feed(ASSISTANT_WITH_USAGE);
        assert!(p.model_usage().is_none());
    }

    #[test]
    fn multi_model_result_picks_largest_context_window() {
        let mut p = StreamParser::new();
        p.feed(RESULT_MULTI_MODEL);
        let usage = p.model_usage().expect("expected model usage");
        assert_eq!(usage.context_window, 1_000_000);
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 80);
    }

    #[test]
    fn model_usage_persists_across_subsequent_feeds() {
        let mut p = StreamParser::new();
        p.feed(RESULT_WITH_MODEL_USAGE);
        assert!(p.model_usage().is_some());
        p.feed(TEXT_LINE);
        assert!(
            p.model_usage().is_some(),
            "model_usage must persist after non-result feed"
        );
    }

    #[test]
    fn format_usage_with_percentage_typical() {
        // 20000 / 200000 = 10.0% — exact, no rounding ambiguity
        assert_eq!(
            format_usage_with_percentage(20000, 200000),
            "20.0k (10.0%) used"
        );
    }

    #[test]
    fn format_usage_with_percentage_zero_tokens() {
        assert_eq!(format_usage_with_percentage(0, 200000), "0.0k (0.0%) used");
    }

    #[test]
    fn format_usage_with_percentage_full_window() {
        assert_eq!(
            format_usage_with_percentage(200000, 200000),
            "200.0k (100.0%) used"
        );
    }

    #[test]
    fn format_usage_with_percentage_zero_context_window() {
        assert_eq!(format_usage_with_percentage(1000, 0), "1.0k (0.0%) used");
    }
}
