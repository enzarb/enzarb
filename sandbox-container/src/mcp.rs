//! The `submit_result` MCP server (stdio), offered to an agent step that
//! declares a `result_schema`.
//!
//! sandbox-init runs it as itself: `sandbox-init mcp-submit-result <schema> <out>`
//! (see [`SUBCOMMAND`]), listed in the step's `session/new` `mcpServers`, so
//! the agent spawns it. It speaks MCP's newline-delimited JSON-RPC on
//! stdin/stdout and offers one tool, `submit_result`, whose input schema is the
//! step's `result_schema`. A call is validated against the schema; errors go
//! back to the agent as a tool error so it can retry, and a valid result is
//! written to `<out>` (the last valid call wins). The agent runs as the same
//! uid and could write `<out>` itself, so sandbox-init validates the file
//! again ([`validate`]) before reporting it.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{Value, json};

/// argv[1] that selects the MCP server mode.
pub const SUBCOMMAND: &str = "mcp-submit-result";
/// The tool name.
pub const TOOL: &str = "submit_result";
/// The MCP server name in `mcpServers`.
pub const SERVER_NAME: &str = "enzarb";

/// The validation errors of `value` against `schema` (empty when valid).
pub fn validate(schema: &Value, value: &Value) -> Vec<String> {
    match jsonschema::validator_for(schema) {
        Ok(v) => v
            .iter_errors(value)
            .map(|e| {
                let path = e.instance_path().to_string();
                if path.is_empty() {
                    e.to_string()
                } else {
                    format!("{path}: {e}")
                }
            })
            .collect(),
        Err(e) => vec![format!("invalid result_schema: {e}")],
    }
}

/// Handle one JSON-RPC message; `None` for notifications.
pub fn handle(schema: &Value, out: &Path, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": [{
            "name": TOOL,
            "description": "Submit the structured result of this step. The input must match the \
                            schema; on validation errors fix them and call again. Call it once \
                            the work is done.",
            "inputSchema": schema,
        }]}),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            if name != TOOL {
                return Some(error(id, -32602, &format!("unknown tool {name:?}")));
            }
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let errors = validate(schema, &args);
            if errors.is_empty() {
                match write_result(out, &args) {
                    Ok(()) => tool_text("Result accepted.", false),
                    Err(e) => tool_text(&format!("cannot store the result: {e}"), true),
                }
            } else {
                tool_text(
                    &format!(
                        "The result does not match the schema:\n- {}",
                        errors.join("\n- ")
                    ),
                    true,
                )
            }
        }
        _ => return Some(error(id, -32601, &format!("method {method:?} not found"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn tool_text(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn write_result(out: &Path, v: &Value) -> std::io::Result<()> {
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(v)?)?;
    std::fs::rename(&tmp, out)
}

/// `sandbox-init mcp-submit-result <schema-file> <out-file>`: serve until stdin closes.
pub fn main(args: &[String]) -> ! {
    let code = match (args.first(), args.get(1)) {
        (Some(schema), Some(out)) => serve(Path::new(schema), Path::new(out)),
        _ => {
            eprintln!("usage: sandbox-init {SUBCOMMAND} <schema.json> <result.json>");
            2
        }
    };
    std::process::exit(code)
}

fn serve(schema: &Path, out: &Path) -> i32 {
    let schema: Value = match std::fs::read(schema)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read the schema: {e}");
            return 1;
        }
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&schema, out, &msg),
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply
            && writeln!(stdout, "{r}").and_then(|()| stdout.flush()).is_err()
        {
            break;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({"type": "object", "required": ["verdict"],
               "properties": {"verdict": {"enum": ["approve", "reject"]}}})
    }

    #[test]
    fn lists_the_tool_and_validates_calls() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("result.json");
        let s = schema();
        let r = handle(&s, &out, &json!({"id": 1, "method": "initialize", "params": {}})).unwrap();
        assert_eq!(r["result"]["capabilities"]["tools"], json!({}));
        let r = handle(&s, &out, &json!({"id": 2, "method": "tools/list"})).unwrap();
        assert_eq!(r["result"]["tools"][0]["name"], TOOL);
        assert_eq!(r["result"]["tools"][0]["inputSchema"], s);
        assert!(handle(&s, &out, &json!({"method": "notifications/initialized"})).is_none());

        let call = |args: Value| {
            handle(
                &s,
                &out,
                &json!({"id": 3, "method": "tools/call", "params": {"name": TOOL, "arguments": args}}),
            )
            .unwrap()
        };
        let r = call(json!({"verdict": "maybe"}));
        assert_eq!(r["result"]["isError"], true);
        assert!(!out.exists());
        let r = call(json!({"verdict": "approve"}));
        assert_eq!(r["result"]["isError"], false);
        let got: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(got, json!({"verdict": "approve"}));
        assert!(validate(&s, &got).is_empty());
        assert!(!validate(&s, &json!({})).is_empty());
    }
}
