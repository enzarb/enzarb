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

fn finished() -> StepFinished {
    StepFinished {
        run_id: "run-123".into(),
        exit: "success".into(),
        outputs: [("plan".to_string(), json!("# Plan"))]
            .into_iter()
            .collect(),
        result: Some(json!({"verdict": "approve"})),
        git: vec![StepGit {
            repo: "enzarb/enzarb".into(),
            branch: "enzarb/issue-42".into(),
            head_sha: "abc123".into(),
            pushed: true,
        }],
        usage: None,
        error: None,
    }
}

fn workspace() -> StepWorkspace {
    StepWorkspace {
        repo: "enzarb/enzarb".into(),
        url: "https://github.com/enzarb/enzarb".into(),
        mode: Mode::Write,
        branch: "enzarb/issue-42".into(),
        base: "main".into(),
        push: true,
        token: Secret::new("ghs_clone"),
    }
}

fn step_start_agent() -> StepStart {
    StepStart {
        run_id: "run-123".into(),
        attempt: 1,
        task_id: "task-1".into(),
        workspaces: vec![workspace()],
        exec: StepExec::Agent {
            runtime: Runtime {
                name: "claude".into(),
                install: vec!["npm:@agentclientprotocol/claude-agent-acp@0.5".into()],
                protocol: "acp".into(),
                command: vec!["claude-agent-acp".into()],
                env: Default::default(),
                capabilities: json!({"sessions": true, "resume": true}),
            },
            model: Some("claude-sonnet-5-5".into()),
            prompt: "Fix the bug \u{1f41b}".into(),
            result_schema: Some(json!({"type": "object"})),
        },
        outputs: [(
            "plan".to_string(),
            StepOutput {
                file: "plan.md".into(),
                format: Some("markdown".into()),
                optional: false,
            },
        )]
        .into(),
        env: [("API_KEY".to_string(), Secret::new("s3cret"))].into(),
        github_token: Some(Secret::new("ghs_step")),
    }
}

fn step_start_command() -> StepStart {
    StepStart {
        exec: StepExec::Command {
            run: "make test".into(),
            tools: vec!["node@22".into()],
        },
        outputs: Default::default(),
        env: Default::default(),
        github_token: None,
        ..step_start_agent()
    }
}

fn ctrl_messages() -> Vec<(&'static str, Ctrl)> {
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
        (
            "step_start_agent",
            Ctrl::StepStart(Box::new(step_start_agent())),
        ),
        (
            "step_start_command",
            Ctrl::StepStart(Box::new(step_start_command())),
        ),
        (
            "step_accepted",
            Ctrl::StepAccepted {
                run_id: "run-123".into(),
                state: StepPhase::Running,
                existing: false,
            },
        ),
        (
            "step_steer",
            Ctrl::StepSteer {
                run_id: "run-123".into(),
                text: "also add tests".into(),
            },
        ),
        (
            "step_stop",
            Ctrl::StepStop {
                run_id: "run-123".into(),
            },
        ),
        (
            "step_resume",
            Ctrl::StepResume {
                run_id: "run-123".into(),
            },
        ),
        (
            "step_status",
            Ctrl::StepStatus {
                run_id: "run-123".into(),
                after_seq: 17,
            },
        ),
        (
            "step_state_running",
            Ctrl::StepState {
                run_id: "run-123".into(),
                state: StepPhase::Running,
                result: None,
            },
        ),
        (
            "step_state_finished",
            Ctrl::StepState {
                run_id: "run-123".into(),
                state: StepPhase::Finished,
                result: Some(finished()),
            },
        ),
        (
            "session_event",
            Ctrl::SessionEvent(SessionEvent {
                run_id: "run-123".into(),
                seq: 1,
                at: "2026-10-16T12:00:00Z".into(),
                kind: "assistant".into(),
                payload: json!({"text": "hi"}),
                truncated: false,
            }),
        ),
        ("step_finished", Ctrl::StepFinished(finished())),
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
        ("stats_request", Ctrl::StatsRequest),
        (
            "stats",
            Ctrl::Stats(SandboxStats {
                ts_ms: 1_760_000_000_000,
                cpu_usage_usec: Some(12_345_678),
                cpu_user_usec: Some(10_000_000),
                cpu_system_usec: Some(2_345_678),
                memory_bytes: Some(268_435_456),
                io_read_bytes: Some(1024),
                io_write_bytes: Some(2048),
                storage_used_bytes: Some(5_000_000_000),
                storage_total_bytes: Some(20_000_000_000),
            }),
        ),
        (
            "stats_partial",
            Ctrl::Stats(SandboxStats {
                ts_ms: 1_760_000_000_000,
                memory_bytes: Some(1),
                ..Default::default()
            }),
        ),
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
    let fin = finished();
    let failed = StepFinished {
        exit: "failure".into(),
        outputs: Default::default(),
        result: None,
        git: vec![],
        error: Some("exit status 1".into()),
        ..fin.clone()
    };
    let with_usage = StepFinished {
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
        ("step_finished", fin),
        ("step_finished_failure", failed),
        ("step_finished_usage", with_usage),
    ];
    let out: Vec<Value> = items
        .iter()
        .map(|(n, v)| json!({"name": n, "type": "StepFinished", "json": serde_json::to_value(v).unwrap()}))
        .collect();
    for (_, v) in &items {
        let s = serde_json::to_string(v).unwrap();
        assert_eq!(&serde_json::from_str::<StepFinished>(&s).unwrap(), v);
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
fn step_start_debug_redacts_secrets() {
    let ss = step_start_agent();
    let d = format!("{ss:?} {:?}", Ctrl::StepStart(Box::new(ss.clone())));
    for secret in ["ghs_clone", "s3cret", "ghs_step"] {
        assert!(!d.contains(secret), "{secret} leaked: {d}");
    }
    assert!(d.contains("run-123"));
    let mut v = ss.secret_values();
    v.sort();
    assert_eq!(v, ["ghs_clone", "ghs_step", "s3cret"]);
}

#[test]
fn step_start_defaults() {
    let v = json!({"run_id": "r", "attempt": 1, "task_id": "t", "workspaces": [],
        "exec": {"kind": "command", "run": "true"}, "future": 1});
    let ss: StepStart = serde_json::from_value(v).unwrap();
    assert!(ss.outputs.is_empty() && ss.env.is_empty() && ss.github_token.is_none());
    assert_eq!(
        ss.exec,
        StepExec::Command {
            run: "true".into(),
            tools: vec![]
        }
    );
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
    }
    for (name, msg) in ctrl_messages() {
        let v = serde_json::to_value(&msg).unwrap();
        let r = serde_json::from_value::<OldCtrl>(v);
        match name {
            "open_claude_token" | "credential_captured_token" | "credential_state_scope" => {
                r.unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            // An old decoder rejects the new message type; that is answered
            // with `unknown_message` (see the session tests).
            "credential_clear" | "step_start_agent" | "step_finished" => assert!(r.is_err()),
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
        json!({"name":"step_state_unknown_fields","payload":r#"{"type":"step_state","run_id":"r","state":"finished","result":{"run_id":"r","exit":"failure","x":2},"y":1}"#,
               "expect":{"type":"step_state","run_id":"r","state":"finished"}}),
        json!({"name":"step_status_no_after_seq","payload":r#"{"type":"step_status","run_id":"r"}"#,
               "expect":{"type":"step_status","run_id":"r"}}),
        json!({"name":"session_event_unknown_kind","payload":r#"{"type":"session_event","run_id":"r","seq":3,"at":"2026-10-16T12:00:00Z","kind":"future_kind","payload":{"a":1}}"#,
               "expect":{"type":"session_event","seq":3,"kind":"future_kind"}}),
        json!({"name":"step_finished_usage_unknown_fields","payload":r#"{"type":"step_finished","run_id":"r","exit":"success","usage":{"input_tokens":5,"future":1}}"#,
               "expect":{"type":"step_finished","run_id":"r","usage":{"input_tokens":5}}}),
        json!({"name":"stats_only_ts","payload":r#"{"type":"stats","ts_ms":5}"#,
               "expect":{"type":"stats","ts_ms":5}}),
        json!({"name":"stats_unknown_fields","payload":r#"{"type":"stats","ts_ms":5,"memory_bytes":7,"gpu_busy":1}"#,
               "expect":{"type":"stats","ts_ms":5,"memory_bytes":7}}),
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
