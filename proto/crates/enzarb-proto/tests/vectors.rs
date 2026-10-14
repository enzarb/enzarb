//! Shared test vectors (`vectors/*.json`). Set `ENZARB_UPDATE_VECTORS=1` to
//! regenerate them from the definitions below; otherwise the files are checked.
//! A few layouts are also pinned by hand-written hex in `codec.rs`.

use bytes::Bytes;
use enzarb_proto::*;
use serde_json::{Value, json};
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors")
}

fn check_or_write(name: &str, v: &Value) {
    let path = dir().join(name);
    let text = serde_json::to_string_pretty(v).unwrap() + "\n";
    if std::env::var_os("ENZARB_UPDATE_VECTORS").is_some() {
        std::fs::write(&path, text).unwrap();
    } else {
        let have = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("{}: {e} (run with ENZARB_UPDATE_VECTORS=1)", path.display())
        });
        assert_eq!(
            serde_json::from_str::<Value>(&have).unwrap(),
            *v,
            "{name} is stale; run with ENZARB_UPDATE_VECTORS=1"
        );
    }
}

fn frame_json(f: &Frame) -> Value {
    json!({
        "type": f.frame_type.name(),
        "flags": f.flags,
        "channel": f.channel,
        "payload_hex": hex::encode(&f.payload),
    })
}

fn run_start() -> RunStart {
    RunStart {
        run_id: "job-123".into(),
        repo_url: "https://github.com/enzarb/enzarb-ui".into(),
        base_ref: "main".into(),
        branch: "enzarb/issue-42".into(),
        push: true,
        clone_token: "ghs_clone".into(),
        prompt: "Fix the bug \u{1f41b}".into(),
        timeout_secs: 3600,
        callback_url: "http://enzarb-workflows-executor.enzarb.svc:8081/v1/runs/job-123/finished"
            .into(),
        callback_token: "cb-secret".into(),
        log_channel: None,
        deps_token: None,
        npm_token: None,
        claude_token: None,
        retarget: None,
    }
}

fn ctrl_messages() -> Vec<(&'static str, Ctrl)> {
    let mut with_log = run_start();
    with_log.log_channel = Some(7);
    let mut with_claude = run_start();
    with_claude.claude_token = Some(Secret::new("sk-ant-oat01-user"));
    vec![
        (
            "hello",
            Ctrl::Hello {
                versions: vec![1],
                client: ClientKind::Ui,
                features: None,
            },
        ),
        (
            "hello_proxy_multi",
            Ctrl::Hello {
                versions: vec![1, 2],
                client: ClientKind::Proxy,
                features: None,
            },
        ),
        (
            "hello_executor",
            Ctrl::Hello {
                versions: vec![1],
                client: ClientKind::Executor,
                features: None,
            },
        ),
        (
            "hello_features",
            Ctrl::Hello {
                versions: vec![1],
                client: ClientKind::Proxy,
                features: Some(vec!["claude_user_token".into()]),
            },
        ),
        (
            "welcome",
            Ctrl::Welcome {
                version: 1,
                features: None,
            },
        ),
        (
            "welcome_features",
            Ctrl::Welcome {
                version: 1,
                features: Some(vec!["acp".into()]),
            },
        ),
        (
            "open",
            Ctrl::Open {
                channel: 1,
                kind: ChannelKind::Pty,
                cols: 80,
                rows: 24,
                command: None,
                repo: None,
                resume: None,
                claude_token: None,
            },
        ),
        (
            "open_acp",
            Ctrl::Open {
                channel: 3,
                kind: ChannelKind::Acp,
                cols: 0,
                rows: 0,
                command: None,
                repo: None,
                resume: None,
                claude_token: None,
            },
        ),
        (
            "open_acp_repo_resume",
            Ctrl::Open {
                channel: 3,
                kind: ChannelKind::Acp,
                cols: 0,
                rows: 0,
                command: None,
                repo: Some("https://github.com/enzarb/enzarb-ui".into()),
                resume: Some("sess_abc123".into()),
                claude_token: None,
            },
        ),
        (
            "open_command",
            Ctrl::Open {
                channel: 2,
                kind: ChannelKind::Pty,
                cols: 120,
                rows: 40,
                command: Some(vec!["bash".into(), "-l".into()]),
                repo: None,
                resume: None,
                claude_token: None,
            },
        ),
        (
            "open_claude_token",
            Ctrl::Open {
                channel: 3,
                kind: ChannelKind::Acp,
                cols: 0,
                rows: 0,
                command: None,
                repo: None,
                resume: None,
                claude_token: Some(Secret::new("sk-ant-oat01-user")),
            },
        ),
        ("opened", Ctrl::Opened { channel: 1 }),
        (
            "open_error",
            Ctrl::OpenError {
                channel: 1,
                message: "too many channels".into(),
                code: None,
            },
        ),
        (
            "open_error_code",
            Ctrl::OpenError {
                channel: 3,
                message: "chat is disabled for this sandbox".into(),
                code: Some("chat_disabled".into()),
            },
        ),
        (
            "resize",
            Ctrl::Resize {
                channel: 1,
                cols: 100,
                rows: 30,
            },
        ),
        ("run_start", Ctrl::RunStart(run_start())),
        ("run_start_log_channel", Ctrl::RunStart(with_log)),
        ("run_start_claude_token", Ctrl::RunStart(with_claude)),
        (
            "run_accepted",
            Ctrl::RunAccepted {
                run_id: "job-123".into(),
                state: RunPhase::Queued,
                existing: false,
            },
        ),
        (
            "run_accepted_existing",
            Ctrl::RunAccepted {
                run_id: "job-123".into(),
                state: RunPhase::Running,
                existing: true,
            },
        ),
        (
            "run_status",
            Ctrl::RunStatus {
                run_id: "job-123".into(),
            },
        ),
        (
            "run_state_running",
            Ctrl::RunState {
                run_id: "job-123".into(),
                state: RunPhase::Running,
                exit_code: None,
                summary: None,
                usage: None,
            },
        ),
        (
            "run_state_done",
            Ctrl::RunState {
                run_id: "job-123".into(),
                state: RunPhase::Succeeded,
                exit_code: Some(0),
                summary: Some("done".into()),
                usage: None,
            },
        ),
        (
            "run_state_failed",
            Ctrl::RunState {
                run_id: "job-123".into(),
                state: RunPhase::Failed,
                exit_code: Some(1),
                summary: None,
                usage: None,
            },
        ),
        (
            "run_state_usage",
            Ctrl::RunState {
                run_id: "job-123".into(),
                state: RunPhase::Succeeded,
                exit_code: Some(0),
                summary: Some("done".into()),
                usage: Some(Usage {
                    input_tokens: Some(1200),
                    output_tokens: Some(340),
                    cache_read_input_tokens: Some(9000),
                    cache_creation_input_tokens: Some(50),
                    models: vec!["claude-sonnet-5-5".into()],
                    cost_usd: None,
                }),
            },
        ),
        (
            "run_cancel",
            Ctrl::RunCancel {
                run_id: "job-123".into(),
            },
        ),
        (
            "credential_captured",
            Ctrl::CredentialCaptured {
                kind: CredentialKind::ClaudeOauth,
                token: None,
                legacy: None,
            },
        ),
        (
            "credential_captured_token",
            Ctrl::CredentialCaptured {
                kind: CredentialKind::ClaudeOauth,
                token: Some(Secret::new("sk-ant-oat01-captured")),
                legacy: Some(true),
            },
        ),
        ("credential_status", Ctrl::CredentialStatus),
        (
            "credential_state",
            Ctrl::CredentialState {
                kind: CredentialKind::ClaudeOauth,
                configured: true,
                scope: None,
            },
        ),
        (
            "credential_state_scope",
            Ctrl::CredentialState {
                kind: CredentialKind::ClaudeOauth,
                configured: true,
                scope: Some(credential_scope::USER.into()),
            },
        ),
        (
            "credential_clear",
            Ctrl::CredentialClear {
                kind: CredentialKind::ClaudeOauth,
            },
        ),
        (
            "error",
            Ctrl::Error {
                code: "unsupported_version".into(),
                message: "no common version".into(),
                run_id: None,
                channel: None,
            },
        ),
        (
            "error_run",
            Ctrl::Error {
                code: "unimplemented".into(),
                message: "no run manager".into(),
                run_id: Some("job-1".into()),
                channel: Some(7),
            },
        ),
    ]
}

#[test]
fn ctrl_vectors() {
    let mut out = vec![];
    for (name, msg) in ctrl_messages() {
        let frame = Frame::ctrl(&msg).unwrap();
        let wire = frame.to_bytes().unwrap();
        out.push(json!({
            "name": name,
            "hex": hex::encode(&wire),
            "message": serde_json::to_value(&msg).unwrap(),
        }));
        // decode side
        let back = Frame::from_bytes(wire).unwrap();
        assert_eq!(back.ctrl_message().unwrap(), msg, "{name}");
    }
    check_or_write("ctrl.json", &Value::Array(out));
}

#[test]
fn frame_vectors() {
    let frames: Vec<(&str, Frame)> = vec![
        (
            "data_bin",
            Frame::data_bin(1, Bytes::from_static(b"\x00\x01\xffhello")),
        ),
        (
            "data_bin_max_channel",
            Frame::data_bin(u32::MAX, Bytes::from_static(b"x")),
        ),
        (
            "data_json",
            Frame::data_json(3, &json!({"line": "hi", "n": 1})).unwrap(),
        ),
        ("window", Frame::window(1, 65536)),
        ("window_max", Frame::window(1, MAX_CREDIT)),
        ("close_empty", Frame::close(1, 0, "")),
        (
            "close_code",
            Frame::close(1, close_code::PROTOCOL_ERROR, "bad frame"),
        ),
        (
            "close_code_only",
            Frame::close(2, close_code::CANCELLED, ""),
        ),
    ];
    let mut out = vec![];
    for (name, f) in frames {
        let wire = f.to_bytes().unwrap();
        out.push(json!({"name": name, "hex": hex::encode(&wire), "frame": frame_json(&f)}));
        assert_eq!(Frame::from_bytes(wire).unwrap(), f, "{name}");
    }
    check_or_write("frames.json", &Value::Array(out));
}

#[test]
fn json_type_vectors() {
    let fin = RunFinished {
        run_id: "job-123".into(),
        outcome: RunOutcome::Succeeded,
        exit_code: Some(0),
        summary: "done".into(),
        branch: "enzarb/issue-42".into(),
        head_sha: "abc123".into(),
        pushed: true,
        usage: None,
    };
    let cancelled = RunFinished {
        outcome: RunOutcome::Cancelled,
        exit_code: None,
        summary: String::new(),
        pushed: false,
        ..fin.clone()
    };
    let with_usage = RunFinished {
        usage: Some(Usage {
            input_tokens: Some(1200),
            output_tokens: Some(340),
            cache_read_input_tokens: Some(9000),
            cache_creation_input_tokens: None,
            models: vec!["claude-sonnet-5-5".into()],
            cost_usd: Some(0.42),
        }),
        ..fin.clone()
    };
    let items = [
        ("run_finished", fin),
        ("run_finished_cancelled", cancelled),
        ("run_finished_usage", with_usage),
    ];
    let out: Vec<Value> = items
        .iter()
        .map(|(n, v)| json!({"name": n, "type": "RunFinished", "json": serde_json::to_value(v).unwrap()}))
        .collect();
    for (_, v) in &items {
        let s = serde_json::to_string(v).unwrap();
        assert_eq!(&serde_json::from_str::<RunFinished>(&s).unwrap(), v);
    }
    check_or_write("json_types.json", &Value::Array(out));
}

#[test]
fn error_vectors() {
    // (name, type, flags, channel, declared length, payload hex, extra trailing hex, error)
    let h = |t: u8, fl: u8, ch: u32, len: u32, payload: &str| {
        format!("{t:02x}{fl:02x}{ch:08x}{len:08x}{payload}")
    };
    let hexs: Vec<(&str, String, &str)> = vec![
        ("truncated_header", "0100".into(), "truncated"),
        ("truncated_payload", h(1, 0, 0, 5, "7b7d"), "truncated"),
        (
            "trailing_bytes",
            h(4, 0, 1, 4, "00000001ff"),
            "trailing_bytes",
        ),
        ("unknown_type_0", h(0, 0, 1, 0, ""), "unknown_type"),
        ("unknown_type_6", h(6, 0, 1, 0, ""), "unknown_type"),
        ("unknown_flags", h(2, 1, 1, 1, "61"), "unknown_flags"),
        (
            "payload_too_large",
            h(2, 0, 1, 0x0010_0001, ""),
            "payload_too_large",
        ),
        (
            "ctrl_nonzero_channel",
            h(1, 0, 1, 2, "7b7d"),
            "ctrl_channel",
        ),
        ("data_bin_channel_zero", h(2, 0, 0, 1, "61"), "zero_channel"),
        (
            "data_json_channel_zero",
            h(3, 0, 0, 2, "7b7d"),
            "zero_channel",
        ),
        (
            "window_channel_zero",
            h(4, 0, 0, 4, "00000001"),
            "zero_channel",
        ),
        ("close_channel_zero", h(5, 0, 0, 0, ""), "zero_channel"),
        ("empty_data_bin", h(2, 0, 1, 0, ""), "empty_data"),
        ("empty_data_json", h(3, 0, 1, 0, ""), "empty_data"),
        ("window_short", h(4, 0, 1, 2, "00ff"), "bad_window_payload"),
        ("window_zero", h(4, 0, 1, 4, "00000000"), "bad_credit"),
        ("window_too_big", h(4, 0, 1, 4, "80000000"), "bad_credit"),
        ("close_one_byte", h(5, 0, 1, 1, "00"), "bad_close_payload"),
        (
            "close_bad_utf8",
            h(5, 0, 1, 3, "0001ff"),
            "bad_close_payload",
        ),
    ];
    let mut out = vec![];
    for (name, h, code) in hexs {
        let err = Frame::from_bytes(Bytes::from(hex::decode(&h).unwrap())).expect_err(name);
        assert_eq!(err.code(), code, "{name}: {err}");
        out.push(json!({"name": name, "hex": h, "error": code}));
    }
    check_or_write("errors.json", &Value::Array(out));
}

#[test]
fn run_start_deps_token_compat() {
    // Old payload without the field decodes to None and re-encodes without it.
    let old = serde_json::to_value(run_start()).unwrap();
    assert!(old.get("deps_token").is_none());
    let back: RunStart = serde_json::from_value(old).unwrap();
    assert_eq!(back.deps_token, None);
    // Round trip with the field set.
    let mut rs = run_start();
    rs.deps_token = Some("ghs_deps".into());
    let v = serde_json::to_value(&rs).unwrap();
    assert_eq!(v["deps_token"], "ghs_deps");
    assert_eq!(serde_json::from_value::<RunStart>(v).unwrap(), rs);
    // Explicit null decodes as None.
    let mut v = serde_json::to_value(run_start()).unwrap();
    v["deps_token"] = serde_json::Value::Null;
    assert_eq!(
        serde_json::from_value::<RunStart>(v).unwrap().deps_token,
        None
    );
}

#[test]
fn run_start_debug_redacts_tokens() {
    let mut rs = run_start();
    rs.deps_token = Some("ghs_deps".into());
    rs.npm_token = Some("ghp_npm".into());
    let d = format!("{rs:?} {:?}", Ctrl::RunStart(rs.clone()));
    for secret in ["ghs_deps", "ghp_npm", "ghs_clone", "cb-secret"] {
        assert!(!d.contains(secret), "{secret} leaked: {d}");
    }
    assert!(d.contains("job-123"));
}

#[test]
fn run_start_npm_token_compat() {
    // Old payload without the field decodes to None and re-encodes without it.
    let old = serde_json::to_value(run_start()).unwrap();
    assert!(old.get("npm_token").is_none());
    assert_eq!(
        serde_json::from_value::<RunStart>(old).unwrap().npm_token,
        None
    );
    // Round trip with the field set; independent of deps_token.
    let mut rs = run_start();
    rs.npm_token = Some("ghp_npm".into());
    let v = serde_json::to_value(&rs).unwrap();
    assert_eq!(v["npm_token"], "ghp_npm");
    assert!(v.get("deps_token").is_none());
    assert_eq!(serde_json::from_value::<RunStart>(v).unwrap(), rs);
    // Explicit null decodes as None.
    let mut v = serde_json::to_value(run_start()).unwrap();
    v["npm_token"] = serde_json::Value::Null;
    assert_eq!(
        serde_json::from_value::<RunStart>(v).unwrap().npm_token,
        None
    );
}

#[test]
fn claude_token_is_a_plain_string_and_old_payloads_decode() {
    let old = serde_json::to_value(run_start()).unwrap();
    assert!(old.get("claude_token").is_none());
    assert_eq!(
        serde_json::from_value::<RunStart>(old)
            .unwrap()
            .claude_token,
        None
    );
    let mut rs = run_start();
    rs.claude_token = Some(Secret::new("sk-ant-oat01-user"));
    let v = serde_json::to_value(&rs).unwrap();
    assert_eq!(v["claude_token"], "sk-ant-oat01-user");
    assert_eq!(serde_json::from_value::<RunStart>(v).unwrap(), rs);
    let d = format!("{rs:?}");
    assert!(!d.contains("sk-ant-oat01-user"), "{d}");
}

#[test]
fn run_start_retarget_compat() {
    let old = serde_json::to_value(run_start()).unwrap();
    assert!(old.get("retarget").is_none());
    assert_eq!(
        serde_json::from_value::<RunStart>(old).unwrap().retarget,
        None
    );
    let mut rs = run_start();
    rs.retarget = Some(enzarb_proto::Retarget {
        head_ref: "enzarb/issue-43".into(),
        onto: "main".into(),
        old_base: "a".repeat(40),
        expected_sha: "b".repeat(40),
        protected: vec![],
    });
    let v = serde_json::to_value(&rs).unwrap();
    assert_eq!(v["retarget"]["head_ref"], "enzarb/issue-43");
    assert!(v["retarget"].get("protected").is_none());
    assert_eq!(serde_json::from_value::<RunStart>(v).unwrap(), rs);
    // `protected` defaults to empty when absent.
    let v = serde_json::json!({"head_ref": "h", "onto": "main",
        "old_base": "a", "expected_sha": "b"});
    let r: enzarb_proto::Retarget = serde_json::from_value(v).unwrap();
    assert!(r.protected.is_empty());
}

/// A decoder that predates the per-user token fields: minimal legacy-shape
/// structs without them must still decode new-style payloads.
#[test]
fn old_decoder_ignores_new_fields() {
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    #[allow(dead_code)]
    enum OldCtrl {
        Open {
            channel: u32,
        },
        CredentialCaptured {
            kind: CredentialKind,
        },
        CredentialState {
            kind: CredentialKind,
            configured: bool,
        },
        RunStart {
            run_id: String,
        },
    }
    for (name, msg) in ctrl_messages() {
        let v = serde_json::to_value(&msg).unwrap();
        let r = serde_json::from_value::<OldCtrl>(v);
        match name {
            "open_claude_token"
            | "credential_captured_token"
            | "credential_state_scope"
            | "run_start_claude_token" => {
                r.unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            // An old decoder rejects the new message type; that is answered
            // with `unknown_message` (see the session tests).
            "credential_clear" => assert!(r.is_err()),
            _ => {}
        }
    }
}

/// Payloads a decoder must accept: old shapes (no new fields) and shapes from
/// newer peers (unknown kind, unknown error code, unknown fields). `expect` is
/// a subset of the decoded message that both languages must agree on.
#[test]
fn ctrl_compat_vectors() {
    let cases = vec![
        json!({"name":"old_welcome","payload":r#"{"type":"welcome","version":1}"#,
               "expect":{"type":"welcome","version":1}}),
        json!({"name":"welcome_features_null","payload":r#"{"type":"welcome","version":1,"features":null}"#,
               "expect":{"type":"welcome","version":1}}),
        json!({"name":"welcome_unknown_feature","payload":r#"{"type":"welcome","version":1,"features":["acp","future_thing"],"x":1}"#,
               "expect":{"type":"welcome","version":1,"features":["acp","future_thing"]}}),
        json!({"name":"hello_unknown_client_feature","payload":r#"{"type":"hello","versions":[1],"client":"proxy","features":["claude_user_token","future_thing"]}"#,
               "expect":{"type":"hello","client":"proxy","features":["claude_user_token","future_thing"]}}),
        json!({"name":"old_open_pty","payload":r#"{"type":"open","channel":1,"kind":"pty","cols":80,"rows":24}"#,
               "expect":{"type":"open","channel":1,"kind":"pty"}}),
        json!({"name":"open_unknown_kind","payload":r#"{"type":"open","channel":5,"kind":"quic","cols":0,"rows":0}"#,
               "expect":{"type":"open","channel":5}}),
        json!({"name":"old_open_error","payload":r#"{"type":"open_error","channel":1,"message":"nope"}"#,
               "expect":{"type":"open_error","channel":1,"message":"nope"}}),
        json!({"name":"open_error_unknown_code","payload":r#"{"type":"open_error","channel":1,"message":"nope","code":"brand_new"}"#,
               "expect":{"type":"open_error","channel":1,"code":"brand_new"}}),
        json!({"name":"old_credential_captured","payload":r#"{"type":"credential_captured","kind":"claude_oauth"}"#,
               "expect":{"type":"credential_captured","kind":"claude_oauth"}}),
        json!({"name":"old_credential_state","payload":r#"{"type":"credential_state","kind":"claude_oauth","configured":false}"#,
               "expect":{"type":"credential_state","kind":"claude_oauth","configured":false}}),
        json!({"name":"credential_state_unknown_scope","payload":r#"{"type":"credential_state","kind":"claude_oauth","configured":true,"scope":"org"}"#,
               "expect":{"type":"credential_state","configured":true,"scope":"org"}}),
        json!({"name":"old_run_state_no_usage","payload":r#"{"type":"run_state","run_id":"r","state":"succeeded","exit_code":0}"#,
               "expect":{"type":"run_state","run_id":"r","state":"succeeded"}}),
        json!({"name":"run_state_usage_unknown_fields","payload":r#"{"type":"run_state","run_id":"r","state":"succeeded","usage":{"input_tokens":5,"future":1}}"#,
               "expect":{"type":"run_state","run_id":"r","usage":{"input_tokens":5}}}),
        json!({"name":"credential_captured_unknown_fields","payload":r#"{"type":"credential_captured","kind":"claude_oauth","token":null,"legacy":null,"x":1}"#,
               "expect":{"type":"credential_captured","kind":"claude_oauth"}}),
    ];
    for c in &cases {
        let f = Frame::new(
            FrameType::Ctrl,
            0,
            Bytes::from(c["payload"].as_str().unwrap().to_owned()),
        );
        let got = serde_json::to_value(f.ctrl_message().unwrap()).unwrap();
        for (k, v) in c["expect"].as_object().unwrap() {
            assert_eq!(&got[k], v, "{} field {k}", c["name"]);
        }
    }
    check_or_write("ctrl_compat.json", &Value::Array(cases));
}
