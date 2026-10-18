use bytes::{Bytes, BytesMut};
use enzarb_proto::*;
use proptest::prelude::*;

#[test]
fn header_layout_is_pinned() {
    let f = Frame::data_bin(0x0102_0304, Bytes::from_static(b"ab"));
    assert_eq!(
        f.to_bytes().unwrap().as_ref(),
        &[
            0x02, 0x00, 0x01, 0x02, 0x03, 0x04, 0x00, 0x00, 0x00, 0x02, b'a', b'b'
        ]
    );
    let w = Frame::window(5, 0x0001_0000);
    assert_eq!(
        w.to_bytes().unwrap().as_ref(),
        &[4, 0, 0, 0, 0, 5, 0, 0, 0, 4, 0, 1, 0, 0]
    );
    let c = Frame::close(9, 1, "x");
    assert_eq!(
        c.to_bytes().unwrap().as_ref(),
        &[5, 0, 0, 0, 0, 9, 0, 0, 0, 3, 0, 1, b'x']
    );
}

#[test]
fn ctrl_hello_wire_is_exact() {
    let f = Frame::ctrl(&Ctrl::Hello {
        versions: vec![1],
        client: ClientKind::Ui,
        features: None,
    })
    .unwrap();
    let json = br#"{"type":"hello","versions":[1],"client":"ui"}"#;
    let mut want = vec![1, 0, 0, 0, 0, 0, 0, 0, 0, json.len() as u8];
    want.extend_from_slice(json);
    assert_eq!(f.to_bytes().unwrap().as_ref(), &want[..]);
}

#[test]
fn encode_rejects_oversize_and_bad_channels() {
    let big = Frame::data_bin(1, vec![0u8; MAX_PAYLOAD_LEN + 1]);
    assert_eq!(big.to_bytes().unwrap_err().code(), "payload_too_large");
    assert!(
        Frame::data_bin(1, vec![0u8; MAX_PAYLOAD_LEN])
            .to_bytes()
            .is_ok()
    );
    assert_eq!(
        Frame::data_bin(0, &b"x"[..]).to_bytes().unwrap_err().code(),
        "zero_channel"
    );
    let c = Frame::new(FrameType::Ctrl, 3, &b"{}"[..]);
    assert_eq!(c.to_bytes().unwrap_err().code(), "ctrl_channel");
}

#[test]
fn decode_is_zero_copy() {
    let wire = Frame::data_bin(1, Bytes::from_static(b"payload"))
        .to_bytes()
        .unwrap();
    let base = wire.as_ptr() as usize;
    let f = Frame::from_bytes(wire).unwrap();
    assert_eq!(f.payload.as_ptr() as usize, base + HEADER_LEN);
}

#[test]
fn streaming_decode_handles_partial_input() {
    let a = Frame::data_bin(1, Bytes::from_static(b"hello"))
        .to_bytes()
        .unwrap();
    let b = Frame::window(1, 10).to_bytes().unwrap();
    let all: Vec<u8> = a.iter().chain(b.iter()).copied().collect();
    let mut buf = BytesMut::new();
    let mut got = vec![];
    for byte in all {
        buf.extend_from_slice(&[byte]);
        while let Some(f) = Frame::decode(&mut buf).unwrap() {
            got.push(f);
        }
    }
    assert_eq!(got.len(), 2);
    assert_eq!(got[1].window_credit().unwrap(), 10);
    assert!(buf.is_empty());
}

#[test]
fn streaming_decode_rejects_bad_header_early() {
    let mut buf = BytesMut::from(&[9u8, 0, 0, 0, 0, 1, 0, 0, 0, 0][..]);
    assert_eq!(Frame::decode(&mut buf).unwrap_err().code(), "unknown_type");
}

#[test]
fn unknown_ctrl_type_and_extra_fields() {
    let f = Frame::new(FrameType::Ctrl, 0, &br#"{"type":"nope"}"#[..]);
    assert_eq!(f.ctrl_message().unwrap_err().code(), "json");
    let f = Frame::new(
        FrameType::Ctrl,
        0,
        &br#"{"type":"welcome","version":1,"future":true}"#[..],
    );
    assert_eq!(
        f.ctrl_message().unwrap(),
        Ctrl::Welcome {
            version: 1,
            features: None
        }
    );
    // optional fields accept explicit null
    let f = Frame::new(
        FrameType::Ctrl,
        0,
        &br#"{"type":"step_state","run_id":"r","state":"running","result":null}"#[..],
    );
    assert!(matches!(
        f.ctrl_message().unwrap(),
        Ctrl::StepState { result: None, .. }
    ));
}

#[test]
fn usage_is_additive_and_unknown_stays_absent() {
    use enzarb_proto::Usage;
    // a step_finished without usage decodes with usage None
    let f = Frame::new(
        FrameType::Ctrl,
        0,
        &br#"{"type":"step_finished","run_id":"r","exit":"success"}"#[..],
    );
    assert!(matches!(
        f.ctrl_message().unwrap(),
        Ctrl::StepFinished(enzarb_proto::step::StepFinished { usage: None, .. })
    ));
    // unknown fields inside usage are ignored; absent fields stay None (not 0)
    let u: Usage = serde_json::from_str(r#"{"input_tokens":3,"future":true}"#).unwrap();
    assert_eq!(u.input_tokens, Some(3));
    assert_eq!(u.output_tokens, None);
    assert_eq!(u.cost_usd, None);
    // absent fields are omitted on the wire; an empty Usage is {}
    assert_eq!(serde_json::to_string(&u).unwrap(), r#"{"input_tokens":3}"#);
    assert_eq!(serde_json::to_string(&Usage::default()).unwrap(), "{}");
    assert!(Usage::default().is_empty());
    // a zero is a value, not unknown
    let z = Usage {
        input_tokens: Some(0),
        ..Default::default()
    };
    assert!(!z.is_empty());
    // StepFinished without usage serialises without the key
    let m = Ctrl::StepFinished(enzarb_proto::step::StepFinished {
        run_id: "r".into(),
        exit: "success".into(),
        outputs: Default::default(),
        result: None,
        git: vec![],
        usage: None,
        environment: None,
        error: None,
    });
    let wire = serde_json::to_string(&m).unwrap();
    assert!(!wire.contains("usage"));
    assert!(!wire.contains("environment"));
}

fn finished_with(environment: Option<enzarb_proto::StepEnvironment>) -> enzarb_proto::StepFinished {
    enzarb_proto::StepFinished {
        run_id: "r".into(),
        exit: "failure".into(),
        outputs: Default::default(),
        result: None,
        git: vec![],
        usage: None,
        environment,
        error: None,
    }
}

#[test]
fn environment_kinds_round_trip_with_exact_wire_strings() {
    use enzarb_proto::{EnvironmentKind as K, StepEnvironment};
    for (kind, wire) in [
        (K::DiskFull, "disk_full"),
        (K::Oom, "oom"),
        (K::ToolchainMissing, "toolchain_missing"),
        (K::Network, "network"),
        (K::Load, "load"),
    ] {
        let m = Ctrl::StepFinished(finished_with(Some(StepEnvironment {
            kind,
            detail: "seen".into(),
        })));
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(
            v["environment"],
            serde_json::json!({"kind": wire, "detail": "seen"})
        );
        assert_eq!(serde_json::from_value::<Ctrl>(v).unwrap(), m);
    }
}

#[test]
fn unknown_environment_kind_decodes_as_unknown() {
    let f = Frame::new(
        FrameType::Ctrl,
        0,
        &br#"{"type":"step_finished","run_id":"r","exit":"failure","outputs":{"a":1},"usage":{"input_tokens":5},"environment":{"kind":"gpu_lost","detail":"gone"}}"#[..],
    );
    let Ctrl::StepFinished(s) = f.ctrl_message().unwrap() else {
        panic!("not step_finished")
    };
    let e = s.environment.unwrap();
    assert_eq!(e.kind, enzarb_proto::EnvironmentKind::Unknown);
    assert_eq!(e.detail, "gone");
    assert_eq!(s.exit, "failure");
    assert_eq!(s.usage.unwrap().input_tokens, Some(5));
    assert_eq!(s.outputs["a"], serde_json::json!(1));
}

/// A decoder that predates `environment` must ignore the field. `OldStepFinished`
/// is a frozen copy of the pre-`environment` struct; do not add fields to it.
/// This fails if `deny_unknown_fields` is ever added to `StepFinished`.
#[test]
fn old_decoder_ignores_environment() {
    use serde::Deserialize;
    use serde_json::{Map, Value};
    #[derive(Deserialize)]
    struct OldStepFinished {
        run_id: String,
        exit: String,
        #[serde(default)]
        outputs: Map<String, Value>,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        git: Vec<enzarb_proto::StepGit>,
        #[serde(default)]
        usage: Option<enzarb_proto::Usage>,
        #[serde(default)]
        error: Option<String>,
    }
    let new = finished_with(Some(enzarb_proto::StepEnvironment {
        kind: enzarb_proto::EnvironmentKind::Oom,
        detail: "killed".into(),
    }));
    let old: OldStepFinished = serde_json::from_value(serde_json::to_value(&new).unwrap()).unwrap();
    assert_eq!(old.run_id, "r");
    assert_eq!(old.exit, "failure");
    assert!(old.outputs.is_empty() && old.result.is_none() && old.git.is_empty());
    assert!(old.usage.is_none() && old.error.is_none());
}

#[test]
fn version_negotiation() {
    assert_eq!(negotiate_version(&[1], SUPPORTED_VERSIONS), Some(1));
    assert_eq!(negotiate_version(&[3, 2, 1], SUPPORTED_VERSIONS), Some(1));
    assert_eq!(negotiate_version(&[2, 3], SUPPORTED_VERSIONS), None);
    assert_eq!(negotiate_version(&[], SUPPORTED_VERSIONS), None);
}

fn arb_frame() -> impl Strategy<Value = Frame> {
    let data = prop::collection::vec(any::<u8>(), 1..2048);
    prop_oneof![
        (1u32.., data.clone()).prop_map(|(c, d)| Frame::data_bin(c, d)),
        (1u32.., data).prop_map(|(c, d)| Frame::new(FrameType::DataJson, c, d)),
        (1u32.., 1..=MAX_CREDIT).prop_map(|(c, n)| Frame::window(c, n)),
        (1u32.., any::<u16>(), "[a-z ]{0,20}").prop_map(|(c, code, r)| Frame::close(c, code, &r)),
        Just(
            Frame::ctrl(&Ctrl::Welcome {
                version: 1,
                features: None
            })
            .unwrap()
        ),
    ]
}

proptest! {
    #[test]
    fn roundtrip(f in arb_frame()) {
        let wire = f.to_bytes().unwrap();
        prop_assert_eq!(wire.len(), f.encoded_len());
        prop_assert_eq!(Frame::from_bytes(wire).unwrap(), f);
    }

    #[test]
    fn stream_roundtrip_any_chunking(frames in prop::collection::vec(arb_frame(), 1..8), chunk in 1usize..64) {
        let mut wire = BytesMut::new();
        for f in &frames { f.encode(&mut wire).unwrap(); }
        let wire = wire.freeze();
        let mut buf = BytesMut::new();
        let mut got = vec![];
        for piece in wire.chunks(chunk) {
            buf.extend_from_slice(piece);
            while let Some(f) = Frame::decode(&mut buf).unwrap() { got.push(f); }
        }
        prop_assert_eq!(got, frames);
        prop_assert!(buf.is_empty());
    }

    #[test]
    fn decode_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = Frame::from_bytes(Bytes::from(bytes.clone()));
        let mut b = BytesMut::from(&bytes[..]);
        let _ = Frame::decode(&mut b);
    }

    #[test]
    fn decoded_frames_reencode_identically(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
        let b = Bytes::from(bytes);
        if let Ok(f) = Frame::from_bytes(b.clone()) {
            prop_assert_eq!(f.to_bytes().unwrap(), b);
        }
    }

    #[test]
    fn ctrl_roundtrip(run_id in "[a-z0-9-]{1,12}", text in ".{0,30}", ch in any::<u32>(), cols in any::<u16>(), rows in any::<u16>()) {
        for m in [
            Ctrl::StepSteer { run_id: run_id.clone(), text },
            Ctrl::Resize { channel: ch, cols, rows },
            Ctrl::Open { channel: ch, kind: ChannelKind::Pty, cols, rows, command: Some(vec![run_id.clone()]), repo: None, resume: None, claude_token: None },
        ] {
            let f = Frame::ctrl(&m).unwrap();
            let back = Frame::from_bytes(f.to_bytes().unwrap()).unwrap();
            prop_assert_eq!(back.ctrl_message().unwrap(), m);
        }
    }
}
