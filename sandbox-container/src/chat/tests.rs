use std::collections::HashSet;

use super::filter::{self, Filter, Out, code};
use super::*;
use crate::config::ChatMode;

const TOKEN: &str = "sk-ant-oat01-SECRETsecretSECRET_0123456789";

fn wt() -> PathBuf {
    PathBuf::from("/workspace/worktrees/chat-0123456789abcdef")
}

fn filter_with(resume: Option<&str>) -> Filter {
    filter_mode(resume, ChatMode::Default, false)
}

fn filter_mode(resume: Option<&str>, mode: ChatMode, allow_bypass: bool) -> Filter {
    Filter::new(
        mode,
        allow_bypass,
        wt(),
        filter::token_forms(TOKEN),
        resume.map(String::from),
    )
}

fn line(v: Value) -> Vec<u8> {
    v.to_string().into_bytes()
}

fn agent_msgs(outs: &[Out]) -> Vec<&Value> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Agent(v) => Some(v),
            _ => None,
        })
        .collect()
}

fn browser_msgs(outs: &[Out]) -> Vec<&Value> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Browser(v) => Some(v),
            _ => None,
        })
        .collect()
}

fn error_code_of(outs: &[Out]) -> Option<i64> {
    browser_msgs(outs)
        .first()
        .and_then(|v| v.pointer("/error/code"))
        .and_then(Value::as_i64)
}

/// initialize (+ response advertising `load`) and session/new (+ response).
fn ready(f: &mut Filter, load: bool) {
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1}}),
    ));
    f.from_agent(&line(json!({"jsonrpc":"2.0","id":0,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":load}}})));
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/","mcpServers":[]}}),
    ));
    let outs = f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":1,"result":{"sessionId":"s-1","modes":{"currentModeId":"default"}}}),
    ));
    assert!(outs.contains(&Out::Session("s-1".into())));
    assert_eq!(f.session(), Some("s-1"));
}

fn permission_request(id: i64, tool_call: Value) -> Vec<u8> {
    line(json!({
        "jsonrpc": "2.0", "id": id, "method": "session/request_permission",
        "params": {
            "sessionId": "s-1",
            "toolCall": tool_call,
            "options": [
                {"optionId": "yes", "name": "Yes", "kind": "allow_once"},
                {"optionId": "always", "name": "Always", "kind": "allow_always"},
                {"optionId": "no", "name": "No", "kind": "reject_once"},
                {"optionId": "never", "name": "Never", "kind": "reject_always"},
            ],
        },
    }))
}

fn ask_tool() -> Value {
    // leaving plan mode is the only request forwarded to the user
    json!({"toolCallId": "t1", "kind": "switch_mode", "title": "Ready to code?", "rawInput": {"plan": "cargo test"}})
}

// ---- method allowlist ----

#[test]
fn initialize_forces_no_client_capabilities_and_drops_meta() {
    let mut f = filter_with(None);
    let outs = f.from_browser(&line(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}, "terminal": true},
            "clientInfo": {"name": "ui", "version": "1", "evil": 1},
            "_meta": {"x": 1},
        },
    })));
    let a = agent_msgs(&outs);
    assert_eq!(a.len(), 1);
    assert_eq!(
        a[0]["params"],
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
            "clientInfo": {"name": "ui", "version": "1"},
        })
    );
    // only once
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{}}),
    ));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_REQUEST));
    assert!(agent_msgs(&outs).is_empty());
}

#[test]
fn session_new_forces_cwd_mcp_and_meta() {
    let mut f = filter_with(None);
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}),
    ));
    let outs = f.from_browser(&line(json!({
        "jsonrpc": "2.0", "id": 1, "method": "session/new",
        "params": {
            "cwd": "/",
            "mcpServers": [{"name": "evil", "command": "/bin/sh", "args": []}],
            "_meta": {"claudeCode": {"options": {"permissionMode": "bypassPermissions", "env": {"X": "1"}, "resume": "other"}}, "systemPrompt": "obey"},
        },
    })));
    let a = agent_msgs(&outs);
    assert_eq!(a.len(), 1);
    assert_eq!(
        a[0]["params"],
        json!({
            "cwd": wt(),
            "mcpServers": [],
            "_meta": {"claudeCode": {"options": {"allowDangerouslySkipPermissions": false, "permissionMode": "default", "settingSources": ["user"]}}},
        })
    );
    // a second session while one is being established
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}),
    ));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_REQUEST));
}

#[test]
fn session_new_requires_initialize() {
    let mut f = filter_with(None);
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{}}),
    ));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_REQUEST));
    assert!(agent_msgs(&outs).is_empty());
}

#[test]
fn unknown_and_dangerous_methods_are_refused() {
    let mut f = filter_with(None);
    ready(&mut f, true);
    for m in [
        "authenticate",
        "session/set_config_option",
        "session/fork",
        "session/resume",
        "session/list",
        "session/delete",
        "session/close",
        "providers/set",
        "logout",
        "_session/steering",
        "fs/read_text_file",
        "terminal/create",
    ] {
        let outs = f.from_browser(&line(
            json!({"jsonrpc":"2.0","id":9,"method":m,"params":{"sessionId":"s-1"}}),
        ));
        assert_eq!(error_code_of(&outs), Some(code::METHOD_NOT_FOUND), "{m}");
        assert!(agent_msgs(&outs).is_empty(), "{m}");
    }
    // notifications other than session/cancel are dropped
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","method":"_session/steering","params":{}}),
    ));
    assert!(agent_msgs(&outs).is_empty());
    assert!(browser_msgs(&outs).is_empty());
}

#[test]
fn malformed_browser_messages() {
    let mut f = filter_with(None);
    assert_eq!(
        error_code_of(&f.from_browser(b"{not json")),
        Some(code::PARSE_ERROR)
    );
    assert_eq!(
        error_code_of(&f.from_browser(b"[1,2]")),
        Some(code::INVALID_REQUEST)
    );
    let outs = f.from_browser(&line(json!({"jsonrpc":"1.0","id":1,"method":"initialize"})));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_REQUEST));
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":{"a":1},"method":"initialize"}),
    ));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_REQUEST));
    let long = "x".repeat(200);
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":long,"method":"initialize"}),
    ));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_REQUEST));
    // a duplicate in-flight id
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":5,"method":"initialize","params":{}}),
    ));
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":5,"method":"session/new","params":{}}),
    ));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_REQUEST));
}

#[test]
fn prompt_rules() {
    let mut f = filter_with(None);
    ready(&mut f, false);
    // wrong session
    let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"other","prompt":[{"type":"text","text":"hi"}]}})));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_PARAMS));
    // image / embedded resource blocks are refused
    for b in [
        json!({"type":"image","data":"AAAA","mimeType":"image/png"}),
        json!({"type":"resource","resource":{"uri":"file:///etc/passwd","text":"x"}}),
    ] {
        let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":4,"method":"session/prompt","params":{"sessionId":"s-1","prompt":[b]}})));
        assert_eq!(error_code_of(&outs), Some(code::INVALID_PARAMS));
    }
    // too large
    let big = "a".repeat(filter::MAX_PROMPT_BYTES + 1);
    let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":5,"method":"session/prompt","params":{"sessionId":"s-1","prompt":[{"type":"text","text":big}]}})));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_PARAMS));
    // ok: rebuilt without unknown fields
    let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":6,"method":"session/prompt","params":{
        "sessionId":"s-1",
        "prompt":[{"type":"text","text":"hi","_meta":{"x":1}},{"type":"resource_link","uri":"file:///a","name":"a","annotations":{}}],
        "_meta":{"steering":{}},
    }})));
    let a = agent_msgs(&outs);
    assert_eq!(
        a[0]["params"],
        json!({"sessionId":"s-1","prompt":[{"type":"text","text":"hi"},{"type":"resource_link","uri":"file:///a","name":"a"}]})
    );
    assert!(f.prompting());
    f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":6,"result":{"stopReason":"end_turn"}}),
    ));
    assert!(!f.prompting());
}

// ---- mode clamp ----

#[test]
fn set_mode_is_clamped_to_default_and_plan() {
    let mut f = filter_with(None);
    ready(&mut f, false);
    for (i, m) in ["default", "plan"].iter().enumerate() {
        let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":10+i,"method":"session/set_mode","params":{"sessionId":"s-1","modeId":m}})));
        assert_eq!(
            agent_msgs(&outs)[0]["params"],
            json!({"sessionId":"s-1","modeId":m})
        );
    }
    for (i, m) in ["bypassPermissions", "acceptEdits", "dontAsk", "", "Default"]
        .iter()
        .enumerate()
    {
        let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":20+i,"method":"session/set_mode","params":{"sessionId":"s-1","modeId":m}})));
        assert_eq!(error_code_of(&outs), Some(code::INVALID_PARAMS), "{m}");
        assert!(agent_msgs(&outs).is_empty(), "{m}");
    }
}

// ---- session/load ----

#[test]
fn load_needs_capability_and_registry() {
    // not advertised
    let mut f = filter_with(Some("s-old"));
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}),
    ));
    f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":0,"result":{"agentCapabilities":{}}}),
    ));
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":"s-old"}}),
    ));
    assert_eq!(error_code_of(&outs), Some(code::METHOD_NOT_FOUND));

    // advertised, but not the registered session
    let mut f = filter_with(Some("s-old"));
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}),
    ));
    f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":0,"result":{"agentCapabilities":{"loadSession":true}}}),
    ));
    for sid in ["s-other", "../../etc", ""] {
        let outs = f.from_browser(&line(
            json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":sid}}),
        ));
        assert_eq!(error_code_of(&outs), Some(code::INVALID_PARAMS), "{sid}");
    }
    // the registered one: cwd and mcpServers forced
    let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"s-old","cwd":"/","mcpServers":[{"name":"x"}]}})));
    assert_eq!(
        agent_msgs(&outs)[0]["params"],
        json!({"sessionId":"s-old","cwd":wt(),"mcpServers":[],"_meta":{"claudeCode":{"options":{"allowDangerouslySkipPermissions":false,"permissionMode":"default","settingSources":["user"]}}}})
    );
    // no mode reported: sandbox-init sets it before the browser sees the session
    let outs = f.from_agent(&line(json!({"jsonrpc":"2.0","id":2,"result":{}})));
    assert!(!outs.contains(&Out::Session("s-old".into())));
    assert!(browser_msgs(&outs).is_empty());
    let set = agent_msgs(&outs)[0].clone();
    assert_eq!(set["method"], "session/set_mode");
    assert_eq!(
        set["params"],
        json!({"sessionId":"s-old","modeId":"default"})
    );
    let outs = f.from_agent(&line(json!({"jsonrpc":"2.0","id":set["id"],"result":{}})));
    assert!(outs.contains(&Out::Session("s-old".into())));
    assert_eq!(browser_msgs(&outs)[0]["id"], 2);

    // without a resume, load is refused
    let mut f = filter_with(None);
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}),
    ));
    f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":0,"result":{"agentCapabilities":{"loadSession":true}}}),
    ));
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":"s-old"}}),
    ));
    assert_eq!(error_code_of(&outs), Some(code::INVALID_PARAMS));
}

#[test]
fn failed_load_allows_a_new_session() {
    let mut f = filter_with(Some("s-old"));
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}),
    ));
    f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":0,"result":{"agentCapabilities":{"loadSession":true}}}),
    ));
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":"s-old"}}),
    ));
    let outs = f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32002,"message":"not found"}}),
    ));
    assert!(!outs.iter().any(|o| matches!(o, Out::Session(_))));
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}),
    ));
    assert_eq!(agent_msgs(&outs).len(), 1);
}

// ---- permissions ----

#[test]
fn leaving_plan_mode_is_forwarded_and_never_auto_approved() {
    let mut f = filter_with(None);
    ready(&mut f, false);
    let outs = f.from_agent(&permission_request(100, ask_tool()));
    assert!(
        agent_msgs(&outs).is_empty(),
        "nothing is answered without the user"
    );
    let b = browser_msgs(&outs);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0]["method"], "session/request_permission");
    assert_eq!(f.pending_permissions(), 1);
}

#[test]
fn permission_answers_are_verified() {
    let cases: Vec<(Value, Value)> = vec![
        // allowed answers pass
        (
            json!({"outcome":{"outcome":"selected","optionId":"yes"}}),
            json!({"outcome":"selected","optionId":"yes"}),
        ),
        (
            json!({"outcome":{"outcome":"selected","optionId":"no"}}),
            json!({"outcome":"selected","optionId":"no"}),
        ),
        (
            json!({"outcome":{"outcome":"cancelled"}}),
            json!({"outcome":"cancelled"}),
        ),
        // always options, unknown ids and junk become reject_once
        (
            json!({"outcome":{"outcome":"selected","optionId":"always"}}),
            json!({"outcome":"selected","optionId":"no"}),
        ),
        (
            json!({"outcome":{"outcome":"selected","optionId":"never"}}),
            json!({"outcome":"selected","optionId":"no"}),
        ),
        (
            json!({"outcome":{"outcome":"selected","optionId":"made-up"}}),
            json!({"outcome":"selected","optionId":"no"}),
        ),
        (
            json!({"outcome":{"outcome":"selected"}}),
            json!({"outcome":"selected","optionId":"no"}),
        ),
        (
            json!({"outcome":"yes"}),
            json!({"outcome":"selected","optionId":"no"}),
        ),
        (json!(null), json!({"outcome":"selected","optionId":"no"})),
    ];
    for (answer, expect) in cases {
        let mut f = filter_with(None);
        ready(&mut f, false);
        f.from_agent(&permission_request(7, ask_tool()));
        let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":7,"result":answer})));
        let a = agent_msgs(&outs);
        assert_eq!(a.len(), 1, "{answer}");
        assert_eq!(a[0]["id"], 7);
        assert_eq!(a[0]["result"]["outcome"], expect, "{answer}");
        // a second (late) answer for the same id is dropped
        let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":7,"result":{"outcome":{"outcome":"selected","optionId":"yes"}}})));
        assert!(agent_msgs(&outs).is_empty());
    }
    // an error answer is a reject
    let mut f = filter_with(None);
    ready(&mut f, false);
    f.from_agent(&permission_request(8, ask_tool()));
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":8,"error":{"code":1,"message":"x"}}),
    ));
    assert_eq!(
        agent_msgs(&outs)[0]["result"]["outcome"],
        json!({"outcome":"selected","optionId":"no"})
    );
}

#[test]
fn answers_for_ids_never_issued_are_dropped() {
    let mut f = filter_with(None);
    ready(&mut f, false);
    for id in [json!(1), json!(999), json!("7"), json!(null)] {
        let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"selected","optionId":"yes"}}})));
        assert!(agent_msgs(&outs).is_empty(), "{id}");
    }
    // "7" (string) is not 7 (number)
    f.from_agent(&permission_request(7, ask_tool()));
    let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":"7","result":{"outcome":{"outcome":"selected","optionId":"yes"}}})));
    assert!(agent_msgs(&outs).is_empty());
    assert_eq!(f.pending_permissions(), 1);
}

#[test]
fn cancel_and_close_resolve_pending_as_cancelled() {
    let mut f = filter_with(None);
    ready(&mut f, false);
    f.from_agent(&permission_request(1, ask_tool()));
    f.from_agent(&permission_request(
        2,
        json!({"toolCallId":"t2","kind":"switch_mode","title":"Plan","rawInput":{"plan": "x"}}),
    ));
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s-1"}}),
    ));
    let a = agent_msgs(&outs);
    assert_eq!(a.len(), 3);
    for m in &a[..2] {
        assert_eq!(m["result"]["outcome"], json!({"outcome":"cancelled"}));
    }
    assert_eq!(a[2]["method"], "session/cancel");
    assert_eq!(f.pending_permissions(), 0);
    // the user's late click does nothing
    let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":1,"result":{"outcome":{"outcome":"selected","optionId":"yes"}}})));
    assert!(agent_msgs(&outs).is_empty());

    f.from_agent(&permission_request(3, ask_tool()));
    let outs = f.cancel_permissions();
    assert_eq!(
        agent_msgs(&outs)[0]["result"]["outcome"],
        json!({"outcome":"cancelled"})
    );
    assert_eq!(
        f.session_cancel().unwrap(),
        json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s-1"}})
    );
}

#[test]
fn every_other_permission_is_allowed_in_the_sandbox() {
    let calls = [
        json!({"toolCallId":"h1","kind":"read","title":"Read","rawInput":{"file_path":"/etc/hosts"}}),
        json!({"toolCallId":"h2","kind":"execute","title":"git push","rawInput":{"command":"git push origin HEAD"}}),
        json!({"toolCallId":"h4","kind":"edit","title":"Edit","rawInput":{"file_path":"/workspace/repos/x/a"}}),
        json!({"toolCallId":"h6","kind":"fetch","title":"fetch","rawInput":{"url":"https://example.com/x"}}),
    ];
    for tc in calls {
        let mut f = filter_with(None);
        ready(&mut f, false);
        let outs = f.from_agent(&permission_request(1, tc.clone()));
        let a = agent_msgs(&outs);
        assert_eq!(a.len(), 1, "{tc}");
        assert_eq!(
            a[0]["result"]["outcome"],
            json!({"outcome":"selected","optionId":"always"}),
            "{tc}"
        );
        assert!(browser_msgs(&outs).is_empty());
        let rec = outs
            .iter()
            .find_map(|o| match o {
                Out::Record("permission", v) => Some(v),
                _ => None,
            })
            .unwrap();
        assert_eq!(rec["decision"], "allow");
        assert_eq!(rec["rule"], "sandbox");
        assert_eq!(f.pending_permissions(), 0);
        // the browser cannot answer it
        let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":1,"result":{"outcome":{"outcome":"selected","optionId":"no"}}})));
        assert!(agent_msgs(&outs).is_empty());
    }
    // without allow_always, allow_once; without any allow option, cancelled
    let mut f = filter_with(None);
    ready(&mut f, false);
    let req = |id: i64, opts: Value| {
        line(
            json!({"jsonrpc":"2.0","id":id,"method":"session/request_permission","params":{
            "sessionId":"s-1",
            "toolCall":{"toolCallId":"x","kind":"execute","rawInput":{"command":"git push"}},
            "options":opts}}),
        )
    };
    let outs = f.from_agent(&req(4, json!([{"optionId":"yes","kind":"allow_once"}])));
    assert_eq!(
        agent_msgs(&outs)[0]["result"]["outcome"],
        json!({"outcome":"selected","optionId":"yes"})
    );
    let outs = f.from_agent(&req(5, json!([{"optionId":"no","kind":"reject_once"}])));
    assert_eq!(
        agent_msgs(&outs)[0]["result"]["outcome"],
        json!({"outcome":"cancelled"})
    );
}

#[test]
fn agent_requests_other_than_permission_are_refused() {
    let mut f = filter_with(None);
    ready(&mut f, false);
    for m in [
        "fs/read_text_file",
        "fs/write_text_file",
        "terminal/create",
        "_enzarb/anything",
        "session/elicitation",
    ] {
        let outs = f.from_agent(&line(
            json!({"jsonrpc":"2.0","id":50,"method":m,"params":{"path":"/etc/passwd"}}),
        ));
        let a = agent_msgs(&outs);
        assert_eq!(a[0]["error"]["code"], code::METHOD_NOT_FOUND, "{m}");
        assert!(browser_msgs(&outs).is_empty(), "{m}");
    }
    // notifications other than session/update are not relayed (no spoofed _enzarb/*)
    let outs = f.from_agent(&line(
        json!({"jsonrpc":"2.0","method":"_enzarb/permission_denied","params":{}}),
    ));
    assert!(browser_msgs(&outs).is_empty());
    // a response to nothing is dropped
    let outs = f.from_agent(&line(json!({"jsonrpc":"2.0","id":4242,"result":{}})));
    assert!(browser_msgs(&outs).is_empty());
}

#[test]
fn permission_flood_is_bounded() {
    let mut f = filter_with(None);
    ready(&mut f, false);
    for i in 0..filter::MAX_PENDING_PERMISSIONS as i64 {
        f.from_agent(&permission_request(1000 + i, ask_tool()));
    }
    let outs = f.from_agent(&permission_request(1, ask_tool()));
    assert_eq!(
        agent_msgs(&outs)[0]["result"]["outcome"],
        json!({"outcome":"cancelled"})
    );
    assert!(browser_msgs(&outs).is_empty());
    // a duplicate id while pending
    let mut f = filter_with(None);
    ready(&mut f, false);
    f.from_agent(&permission_request(5, ask_tool()));
    let outs = f.from_agent(&permission_request(5, ask_tool()));
    assert_eq!(
        agent_msgs(&outs)[0]["result"]["outcome"],
        json!({"outcome":"cancelled"})
    );
}

// ---- redaction ----

#[test]
fn token_is_redacted_from_everything_sent_to_the_browser() {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(TOKEN);
    let mut f = filter_with(None);
    ready(&mut f, false);
    let leaky = format!("token={TOKEN} b64={b64} other=sk-ant-api03-ABCDEFGHIJKLMNOP");
    let outs = f.from_agent(&line(json!({
        "jsonrpc":"2.0","method":"session/update",
        "params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":leaky}, TOKEN: "key"}},
    })));
    let outs2 = f.from_agent(&permission_request(9, json!({"toolCallId":"t","kind":"execute","title":TOKEN,"rawInput":{"command":format!("echo {TOKEN}")}})));
    let outs3 = f.from_agent(&permission_request(10, json!({"toolCallId":"t","kind":"read","title":"x","rawInput":{"file_path":format!("/workspace/.enzarb/{TOKEN}")}})));
    for o in outs.iter().chain(&outs2).chain(&outs3) {
        if let Out::Browser(v) | Out::Record(_, v) = o {
            let s = v.to_string();
            assert!(!s.contains("SECRETsecretSECRET"), "leaked: {s}");
            assert!(!s.contains(&b64[..b64.len() - 4]), "leaked b64: {s}");
            assert!(!s.contains("ABCDEFGHIJKLMNOP"), "leaked other token: {s}");
        }
    }
    // responses too
    f.from_browser(&line(json!({"jsonrpc":"2.0","id":11,"method":"session/set_mode","params":{"sessionId":"s-1","modeId":"plan"}})));
    let outs = f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":11,"error":{"code":1,"message":TOKEN}}),
    ));
    assert!(!browser_msgs(&outs)[0].to_string().contains("SECRETsecret"));
}

#[test]
fn redact_str_keeps_short_prefixes_and_other_text() {
    assert_eq!(
        filter::redact_str("sk-ant- is a prefix", &[]),
        "sk-ant- is a prefix"
    );
    assert_eq!(filter::redact_str("x sk-ant-abc y", &[]), "x sk-ant-abc y");
    assert_eq!(
        filter::redact_str("x sk-ant-oat01-abcdefgh y", &[]),
        "x [redacted] y"
    );
    assert_eq!(
        filter::redact_str("plain", &["secretvalue".into()]),
        "plain"
    );
    assert_eq!(
        filter::redact_str("a secretvalue b", &["secretvalue".into()]),
        "a [redacted] b"
    );
}

// ---- line cap ----

#[test]
fn line_buffer_splits_and_caps() {
    let mut b = LineBuf::new(10);
    assert_eq!(b.push(b"ab").unwrap(), Vec::<Vec<u8>>::new());
    assert_eq!(
        b.push(b"c\n\n  \nde\nf").unwrap(),
        vec![b"abc".to_vec(), b"de".to_vec()]
    );
    assert_eq!(b.push(b"1234567890").unwrap_err(), LineTooLong);
    let mut b = LineBuf::new(10);
    assert!(b.push(b"0123456789\n").is_ok());
    assert!(b.push(b"0123456789A\n").is_err());
    let mut b = LineBuf::new(MAX_LINE);
    let big = vec![b'x'; MAX_LINE + 1];
    assert!(b.push(&big).is_err());
}

// ---- open validation ----

#[test]
fn repo_and_resume_validation() {
    assert!(valid_repo("https://github.com/enzarb/enzarb-ui.git"));
    assert!(valid_repo("git@github.com:enzarb/x.git"));
    assert!(!valid_repo(""));
    assert!(!valid_repo("https://x/ y"));
    assert!(!valid_repo("https://x/\ny"));
    assert!(!valid_repo("https://x/\u{e9}"));
    assert!(!valid_repo(&"a".repeat(MAX_REPO_LEN + 1)));
    // whatever the URL, the clone dir is a single sanitized component
    for url in [
        "../../etc",
        "/etc/passwd",
        "file:///workspace/.enzarb",
        "a/../../b",
    ] {
        let n = repo_dir_name(url);
        assert!(!n.contains('/') && !n.starts_with('.'), "{url} -> {n}");
    }
    assert!(valid_resume("0b5a4f0e-1c2d-4e5f-8a9b-0c1d2e3f4a5b"));
    assert!(valid_resume("sess_1.2"));
    for bad in [
        "",
        ".hidden",
        "../x",
        "a/b",
        "a b",
        "a\0b",
        &"a".repeat(129),
    ] {
        assert!(!valid_resume(bad), "{bad:?}");
    }
    assert!(valid_chat_id(&new_chat_id()));
    assert_ne!(new_chat_id(), new_chat_id());
    assert!(!valid_chat_id("0123456789ABCDEF"));
    assert!(!valid_chat_id("../0123456789abc"));
}

#[test]
fn close_reasons() {
    assert_eq!(End::Reason(close_reason::IDLE).reason(), "idle");
    assert_eq!(End::Reason(close_reason::LIMIT).reason(), "limit");
    assert_eq!(End::Exited(Some(ExitStatus::Exited(3))).reason(), "exit:3");
    assert_eq!(
        End::Exited(Some(ExitStatus::Signaled(9))).reason(),
        "signal:9"
    );
    assert_eq!(End::BrowserClosed.reason(), "");
}

#[test]
fn agent_env_is_minimal() {
    let cfg = Config::default();
    let m = ChatManager::new(cfg, Reaper::new(false), RepoLocks::default());
    let env = m.agent_env("sk-ant-oat01-x", Path::new("/w/.enzarb/chat-home/.claude"));
    let allowed: HashSet<&str> = PASS_ENV
        .iter()
        .copied()
        .chain([
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CONFIG_DIR",
            "GIT_TERMINAL_PROMPT",
            "GIT_AUTHOR_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_NAME",
            "GIT_COMMITTER_EMAIL",
        ])
        .collect();
    for (k, _) in &env {
        assert!(allowed.contains(k.as_str()), "{k}");
    }
    for k in [
        "RUSTC_WRAPPER",
        "SCCACHE_DIR",
        "SCCACHE_CACHE_SIZE",
        "npm_config_cache",
        "PLAYWRIGHT_BROWSERS_PATH",
        "PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD",
    ] {
        assert!(PASS_ENV.contains(&k), "{k} must reach agents");
    }
    let get = |k: &str| env.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    assert_eq!(get("CLAUDE_CODE_OAUTH_TOKEN"), Some("sk-ant-oat01-x"));
    assert_eq!(
        get("CLAUDE_CONFIG_DIR"),
        Some("/w/.enzarb/chat-home/.claude")
    );
    for k in [
        "GIT_CONFIG_COUNT",
        "ENZARB_DEPS_TOKEN",
        "NODE_AUTH_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        assert_eq!(get(k), None);
    }
}

#[test]
fn disabled_chat_has_no_feature() {
    let cfg = Config {
        max_chats: 0,
        acp_bin: "/bin/sh".into(),
        ..Config::default()
    };
    let m = ChatManager::new(cfg, Reaper::new(false), RepoLocks::default());
    assert!(!m.enabled());
    assert!(m.features().is_empty());
    let cfg = Config {
        acp_bin: "/nonexistent/claude-agent-acp".into(),
        ..Config::default()
    };
    assert!(
        ChatManager::new(cfg, Reaper::new(false), RepoLocks::default())
            .features()
            .is_empty()
    );
    let cfg = Config {
        acp_bin: "/bin/sh".into(),
        ..Config::default()
    };
    assert_eq!(
        ChatManager::new(cfg, Reaper::new(false), RepoLocks::default()).features(),
        vec!["acp".to_string()]
    );
}

#[test]
fn ids_must_round_trip_through_js() {
    let mut f = filter_with(None);
    for id in [
        json!(1.0),
        json!(1e3),
        json!(9007199254740993u64),
        json!(true),
    ] {
        let outs = f.from_browser(&line(
            json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{}}),
        ));
        assert_eq!(error_code_of(&outs), Some(code::INVALID_REQUEST), "{id}");
        assert!(agent_msgs(&outs).is_empty());
    }
}

#[test]
fn duplicate_option_ids_are_cancelled() {
    let mut f = filter_with(None);
    ready(&mut f, false);
    let outs = f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":3,"method":"session/request_permission","params":{
        "sessionId":"s-1","toolCall":ask_tool(),
        "options":[{"optionId":"x","kind":"allow_always"},{"optionId":"x","kind":"allow_once"}]}}),
    ));
    assert_eq!(
        agent_msgs(&outs)[0]["result"]["outcome"],
        json!({"outcome":"cancelled"})
    );
    assert!(browser_msgs(&outs).is_empty());
}

#[test]
fn tampered_meta_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().join("0123456789abcdef");
    std::fs::create_dir(&d).unwrap();
    let write = |m: Value| std::fs::write(d.join("meta.json"), m.to_string()).unwrap();
    let base = json!({"chat_id":"0123456789abcdef","clone":"x-1234","worktree":"/w","acp_session_id":"s","created":0,"last_active":0});
    write(base.clone());
    assert!(read_meta(&d).is_some());
    for (k, v) in [
        ("clone", json!("../worktrees")),
        ("clone", json!("/etc")),
        ("clone", json!(".git")),
        ("acp_session_id", json!("../x")),
        ("chat_id", json!("fedcba9876543210")),
    ] {
        let mut m = base.clone();
        m[k] = v;
        write(m);
        assert!(read_meta(&d).is_none(), "{k}");
    }
}

// ---- permission mode ----

fn init(f: &mut Filter) {
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}),
    ));
    f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":0,"result":{"agentCapabilities":{"loadSession":true}}}),
    ));
}

fn meta(allow: bool, mode: &str) -> Value {
    json!({"claudeCode": {"options": {"allowDangerouslySkipPermissions": allow, "permissionMode": mode, "settingSources": ["user"]}}})
}

#[test]
fn session_meta_follows_the_configured_mode_not_the_browser() {
    let cases = [
        (ChatMode::Bypass, true, meta(true, "bypassPermissions")),
        // bypass mode implies bypass allowed
        (ChatMode::Bypass, false, meta(true, "bypassPermissions")),
        (ChatMode::Default, true, meta(true, "default")),
        (ChatMode::Default, false, meta(false, "default")),
        (ChatMode::Plan, false, meta(false, "plan")),
    ];
    for (mode, allow, want) in cases {
        for browser_meta in [
            json!({"claudeCode": {"options": {"permissionMode": "acceptEdits", "allowDangerouslySkipPermissions": !allow, "settingSources": ["project"]}}}),
            json!({"claudeCode": {"options": {"permissionMode": "default", "allowDangerouslySkipPermissions": false}}}),
        ] {
            let mut f = filter_mode(Some("s-old"), mode, allow);
            init(&mut f);
            let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/","_meta":browser_meta}})));
            assert_eq!(agent_msgs(&outs)[0]["params"]["_meta"], want, "{mode:?}");
            let mut f = filter_mode(Some("s-old"), mode, allow);
            init(&mut f);
            let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":"s-old","_meta":browser_meta}})));
            assert_eq!(agent_msgs(&outs)[0]["params"]["_meta"], want, "{mode:?}");
        }
    }
}

#[test]
fn bypass_mode_is_enforced_before_the_browser_gets_the_session() {
    let mut f = filter_mode(None, ChatMode::Bypass, true);
    init(&mut f);
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/"}}),
    ));
    let modes = json!({"currentModeId":"acceptEdits","availableModes":[{"id":"default"},{"id":"acceptEdits"},{"id":"bypassPermissions"}]});
    let outs = f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":1,"result":{"sessionId":"s-1","modes":modes}}),
    ));
    assert!(browser_msgs(&outs).is_empty());
    assert_eq!(f.session(), None);
    let set = agent_msgs(&outs)[0].clone();
    assert_eq!(
        set["params"],
        json!({"sessionId":"s-1","modeId":"bypassPermissions"})
    );
    // the browser cannot use (or prompt on) the session meanwhile, nor reuse the id
    let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":set["id"],"method":"session/prompt","params":{"sessionId":"s-1","prompt":[{"type":"text","text":"x"}]}})));
    assert!(agent_msgs(&outs).is_empty());
    let outs = f.from_agent(&line(json!({"jsonrpc":"2.0","id":set["id"],"result":{}})));
    assert!(outs.contains(&Out::Session("s-1".into())));
    let b = browser_msgs(&outs);
    assert_eq!(b[0]["id"], 1);
    assert_eq!(
        b[0]["result"]["modes"]["currentModeId"],
        "bypassPermissions"
    );
    // availableModes passes unchanged
    assert_eq!(
        b[0]["result"]["modes"]["availableModes"],
        modes["availableModes"]
    );
    assert_eq!(b[1]["method"], filter::MODE);
    assert_eq!(
        b[1]["params"],
        json!({"sessionId":"s-1","modeId":"bypassPermissions","bypassAllowed":true})
    );
}

#[test]
fn matching_mode_is_not_switched() {
    let mut f = filter_mode(None, ChatMode::Bypass, true);
    init(&mut f);
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{}}),
    ));
    let outs = f.from_agent(&line(json!({"jsonrpc":"2.0","id":1,"result":{"sessionId":"s-1","modes":{"currentModeId":"bypassPermissions"}}})));
    assert!(agent_msgs(&outs).is_empty());
    assert!(outs.contains(&Out::Session("s-1".into())));
    let b = browser_msgs(&outs);
    assert_eq!(b[0]["id"], 1);
    assert_eq!(b[1]["method"], filter::MODE);
}

#[test]
fn refused_mode_means_no_session() {
    let mut f = filter_mode(None, ChatMode::Bypass, true);
    init(&mut f);
    f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":"n","method":"session/new","params":{}}),
    ));
    let outs = f.from_agent(&line(json!({"jsonrpc":"2.0","id":"n","result":{"sessionId":"s-1","modes":{"currentModeId":"default"}}})));
    let set = agent_msgs(&outs)[0].clone();
    let outs = f.from_agent(&line(
        json!({"jsonrpc":"2.0","id":set["id"],"error":{"code":-32602,"message":"Invalid Mode"}}),
    ));
    assert_eq!(f.session(), None);
    let b = browser_msgs(&outs);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0]["id"], "n");
    assert_eq!(b[0]["error"]["code"], code::INTERNAL_ERROR);
    // a retried session/new is forwarded
    let outs = f.from_browser(&line(
        json!({"jsonrpc":"2.0","id":"n2","method":"session/new","params":{}}),
    ));
    assert_eq!(agent_msgs(&outs).len(), 1);
}

#[test]
fn set_mode_matrix() {
    for (mode, allow) in [
        (ChatMode::Bypass, true),
        (ChatMode::Default, true),
        (ChatMode::Default, false),
        (ChatMode::Plan, false),
    ] {
        let mut f = filter_mode(None, mode, allow);
        init(&mut f);
        f.from_browser(&line(
            json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{}}),
        ));
        f.from_agent(&line(json!({"jsonrpc":"2.0","id":1,"result":{"sessionId":"s-1","modes":{"currentModeId":mode.as_str()}}})));
        let bypass_ok = allow || mode == ChatMode::Bypass;
        for (i, (m, ok)) in [
            ("bypassPermissions", bypass_ok),
            ("default", true),
            ("plan", true),
            ("acceptEdits", false),
            ("auto", false),
            ("dontAsk", false),
            ("", false),
            ("Default", false),
            ("bypass", false),
        ]
        .into_iter()
        .enumerate()
        {
            let id = 10 + i as i64;
            let outs = f.from_browser(&line(json!({"jsonrpc":"2.0","id":id,"method":"session/set_mode","params":{"sessionId":"s-1","modeId":m}})));
            if !ok {
                assert_eq!(
                    error_code_of(&outs),
                    Some(code::INVALID_PARAMS),
                    "{mode:?} {m}"
                );
                assert!(agent_msgs(&outs).is_empty());
                continue;
            }
            assert_eq!(
                agent_msgs(&outs)[0]["params"],
                json!({"sessionId":"s-1","modeId":m})
            );
            let outs = f.from_agent(&line(json!({"jsonrpc":"2.0","id":id,"result":{}})));
            let b = browser_msgs(&outs);
            assert_eq!(b[0]["id"], id);
            assert_eq!(b[1]["method"], filter::MODE);
            assert_eq!(b[1]["params"]["modeId"], m);
        }
        // a failed switch sends no mode notification
        f.from_browser(&line(json!({"jsonrpc":"2.0","id":99,"method":"session/set_mode","params":{"sessionId":"s-1","modeId":"plan"}})));
        let outs = f.from_agent(&line(
            json!({"jsonrpc":"2.0","id":99,"error":{"code":1,"message":"no"}}),
        ));
        assert_eq!(browser_msgs(&outs).len(), 1);
    }
}
