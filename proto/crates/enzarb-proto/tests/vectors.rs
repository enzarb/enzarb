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
    }
}

fn ctrl_messages() -> Vec<(&'static str, Ctrl)> {
    let mut with_log = run_start();
    with_log.log_channel = Some(7);
    vec![
        (
            "hello",
            Ctrl::Hello {
                versions: vec![1],
                client: ClientKind::Ui,
            },
        ),
        (
            "hello_proxy_multi",
            Ctrl::Hello {
                versions: vec![1, 2],
                client: ClientKind::Proxy,
            },
        ),
        (
            "hello_executor",
            Ctrl::Hello {
                versions: vec![1],
                client: ClientKind::Executor,
            },
        ),
        ("welcome", Ctrl::Welcome { version: 1 }),
        (
            "open",
            Ctrl::Open {
                channel: 1,
                kind: ChannelKind::Pty,
                cols: 80,
                rows: 24,
                command: None,
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
            },
        ),
        ("opened", Ctrl::Opened { channel: 1 }),
        (
            "open_error",
            Ctrl::OpenError {
                channel: 1,
                message: "too many channels".into(),
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
            },
        ),
        (
            "run_state_done",
            Ctrl::RunState {
                run_id: "job-123".into(),
                state: RunPhase::Succeeded,
                exit_code: Some(0),
                summary: Some("done".into()),
            },
        ),
        (
            "run_state_failed",
            Ctrl::RunState {
                run_id: "job-123".into(),
                state: RunPhase::Failed,
                exit_code: Some(1),
                summary: None,
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
            },
        ),
        ("credential_status", Ctrl::CredentialStatus),
        (
            "credential_state",
            Ctrl::CredentialState {
                kind: CredentialKind::ClaudeOauth,
                configured: true,
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
    };
    let cancelled = RunFinished {
        outcome: RunOutcome::Cancelled,
        exit_code: None,
        summary: String::new(),
        pushed: false,
        ..fin.clone()
    };
    let items = [("run_finished", fin), ("run_finished_cancelled", cancelled)];
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
