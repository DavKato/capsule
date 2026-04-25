use crate::verdict::{Verdict, VerdictStatus};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

/// Handle a `tools/call` for `submit_verdict`. Returns a JSON object
/// `{ok: true, verdict: {...}}` or `{ok: false, error: "..."}`.
pub fn handle_verdict_call(args: &Value) -> Value {
    let status_str = match args.get("status").and_then(Value::as_str) {
        Some(s) => s,
        None => return json!({"ok": false, "error": "missing or non-string 'status' field"}),
    };
    let status = match status_str {
        "pass" => VerdictStatus::Pass,
        "fail" => VerdictStatus::Fail,
        "done" => VerdictStatus::Done,
        other => {
            return json!({"ok": false, "error": format!("unknown status: {other}")});
        }
    };
    let notes = args.get("notes").and_then(Value::as_str).map(str::to_owned);
    let verdict = Verdict { status, notes };
    json!({"ok": true, "verdict": verdict})
}

/// Handle one JSON-RPC line. Returns `None` for notifications (no response),
/// `Some(response_json)` for requests.
pub fn handle_message(line: &str) -> Option<String> {
    let msg: Value = serde_json::from_str(line).ok()?;
    let method = msg.get("method")?.as_str()?;
    let id = msg.get("id")?;

    let result: Value = match method {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "capsule", "version": env!("CARGO_PKG_VERSION")}
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "submit_verdict",
                "description": "Signal stage completion with a pass or fail verdict.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "status": {"type": "string", "enum": ["pass", "fail", "done"]},
                        "notes": {"type": "string"}
                    },
                    "required": ["status"]
                }
            }]
        }),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            if name != "submit_verdict" {
                json!({"content": [{"type": "text", "text": r#"{"ok":false,"error":"unknown tool"}"#}]})
            } else {
                let args = msg
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                let res = handle_verdict_call(&args);
                let text = serde_json::to_string(&res).unwrap_or_default();
                json!({"content": [{"type": "text", "text": text}]})
            }
        }
        _ => {
            let response = json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": "Method not found"}
            });
            return Some(serde_json::to_string(&response).unwrap_or_default());
        }
    };

    let response = json!({"jsonrpc": "2.0", "id": id, "result": result});
    Some(serde_json::to_string(&response).unwrap_or_default())
}

/// Read JSON-RPC messages from stdin, write responses to stdout.
pub fn run_server() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if let Some(response) = handle_message(&line) {
            let _ = writeln!(out, "{response}");
            let _ = out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_pass_returns_ok_with_verdict() {
        let args = json!({"status": "pass"});
        let res = handle_verdict_call(&args);
        assert_eq!(res["ok"], true);
        assert_eq!(res["verdict"]["status"], "pass");
        assert!(res["verdict"]["notes"].is_null());
    }

    #[test]
    fn valid_pass_with_notes() {
        let args = json!({"status": "pass", "notes": "all tests green"});
        let res = handle_verdict_call(&args);
        assert_eq!(res["ok"], true);
        assert_eq!(res["verdict"]["notes"], "all tests green");
    }

    #[test]
    fn valid_fail_returns_ok() {
        let args = json!({"status": "fail", "notes": "build broke"});
        let res = handle_verdict_call(&args);
        assert_eq!(res["ok"], true);
        assert_eq!(res["verdict"]["status"], "fail");
    }

    #[test]
    fn valid_done_returns_ok_with_done_verdict() {
        let args = json!({"status": "done", "notes": "loop exited"});
        let res = handle_verdict_call(&args);
        assert_eq!(res["ok"], true);
        assert_eq!(res["verdict"]["status"], "done");
        assert_eq!(res["verdict"]["notes"], "loop exited");
    }

    #[test]
    fn unknown_status_returns_error() {
        let args = json!({"status": "unknown"});
        let res = handle_verdict_call(&args);
        assert_eq!(res["ok"], false);
        assert!(res["error"].as_str().unwrap().contains("unknown status"));
    }

    #[test]
    fn missing_status_returns_error() {
        let args = json!({"notes": "oops"});
        let res = handle_verdict_call(&args);
        assert_eq!(res["ok"], false);
        assert!(res["error"].as_str().unwrap().contains("missing"));
    }

    #[test]
    fn wrong_type_for_status_returns_error() {
        let args = json!({"status": 42});
        let res = handle_verdict_call(&args);
        assert_eq!(res["ok"], false);
    }

    #[test]
    fn initialize_returns_protocol_version() {
        let req = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#;
        let res = handle_message(req).unwrap();
        let v: Value = serde_json::from_str(&res).unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn tools_list_returns_submit_verdict() {
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let res = handle_message(req).unwrap();
        let v: Value = serde_json::from_str(&res).unwrap();
        assert_eq!(v["result"]["tools"][0]["name"], "submit_verdict");
    }

    #[test]
    fn tools_list_includes_done_in_status_enum() {
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let res = handle_message(req).unwrap();
        let v: Value = serde_json::from_str(&res).unwrap();
        let status_enum = &v["result"]["tools"][0]["inputSchema"]["properties"]["status"]["enum"];
        let values: Vec<&str> = status_enum
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert!(values.contains(&"pass"));
        assert!(values.contains(&"fail"));
        assert!(values.contains(&"done"));
    }

    #[test]
    fn tools_call_submit_verdict_pass() {
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"submit_verdict","arguments":{"status":"pass"}}}"#;
        let res = handle_message(req).unwrap();
        let v: Value = serde_json::from_str(&res).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        let inner: Value = serde_json::from_str(text).unwrap();
        assert_eq!(inner["ok"], true);
        assert_eq!(inner["verdict"]["status"], "pass");
    }

    #[test]
    fn tools_call_bad_status_returns_ok_false() {
        let req = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"submit_verdict","arguments":{"status":"bad"}}}"#;
        let res = handle_message(req).unwrap();
        let v: Value = serde_json::from_str(&res).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        let inner: Value = serde_json::from_str(text).unwrap();
        assert_eq!(inner["ok"], false);
    }

    #[test]
    fn unknown_method_with_id_returns_error() {
        let req = r#"{"jsonrpc":"2.0","id":4,"method":"unknown/method","params":{}}"#;
        let res = handle_message(req).unwrap();
        let v: Value = serde_json::from_str(&res).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }

    #[test]
    fn notification_no_id_returns_none() {
        let req = r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#;
        assert!(handle_message(req).is_none());
    }

    #[test]
    fn malformed_call_does_not_produce_verdict() {
        // ok:false responses from mcp_server must not carry a Verdict field.
        let bad_status = json!({"status": "bogus"});
        let res = handle_verdict_call(&bad_status);
        assert_eq!(res["ok"], false);
        assert!(res.get("verdict").is_none() || res["verdict"].is_null());

        let missing_status = json!({"notes": "oops"});
        let res = handle_verdict_call(&missing_status);
        assert_eq!(res["ok"], false);
        assert!(res.get("verdict").is_none() || res["verdict"].is_null());
    }
}
