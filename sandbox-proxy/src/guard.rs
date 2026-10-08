//! Security boundary: what a browser may send towards the sandbox.
//!
//! Only the workflows executor may start, cancel or poll runs. The proxy
//! therefore fully decodes every browser frame and applies an allowlist to CTRL
//! messages. Anything unparseable is rejected too (fail closed), so there is no
//! way to smuggle a `run_*` message past the proxy with a malformed envelope
//! that the sandbox would still interpret.

use bytes::Bytes;
use enzarb_proto::{Ctrl, Frame, FrameType};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Reject {
    #[error("malformed frame: {0}")]
    Malformed(String),
    #[error("CTRL message not permitted from a browser: {0}")]
    ForbiddenCtrl(&'static str),
}

/// Classification of an allowed frame, for logging/metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allowed {
    pub frame_type: FrameType,
    pub channel: u32,
    pub payload_len: usize,
}

/// CTRL messages a browser (`client: "ui"`) may send after the handshake.
fn ctrl_allowed(m: &Ctrl) -> Result<(), &'static str> {
    match m {
        Ctrl::Open { .. } | Ctrl::Resize { .. } | Ctrl::Error { .. } => Ok(()),
        Ctrl::Hello { .. } => Err("hello"),
        Ctrl::Welcome { .. } => Err("welcome"),
        Ctrl::Opened { .. } => Err("opened"),
        Ctrl::OpenError { .. } => Err("open_error"),
        Ctrl::RunStart(_) => Err("run_start"),
        Ctrl::RunAccepted { .. } => Err("run_accepted"),
        Ctrl::RunStatus { .. } => Err("run_status"),
        Ctrl::RunState { .. } => Err("run_state"),
        Ctrl::RunCancel { .. } => Err("run_cancel"),
        Ctrl::CredentialCaptured { .. } => Err("credential_captured"),
    }
}

/// Validate one browser->sandbox websocket binary message (exactly one frame).
pub fn check_browser_message(msg: &Bytes) -> Result<Allowed, Reject> {
    let frame =
        Frame::from_bytes(msg.clone()).map_err(|e| Reject::Malformed(e.code().to_string()))?;
    if frame.frame_type == FrameType::Ctrl {
        let ctrl = frame
            .ctrl_message()
            .map_err(|e| Reject::Malformed(e.code().to_string()))?;
        ctrl_allowed(&ctrl).map_err(Reject::ForbiddenCtrl)?;
    }
    Ok(Allowed {
        frame_type: frame.frame_type,
        channel: frame.channel,
        payload_len: frame.payload.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_proto::{ChannelKind, RunPhase, RunStart};

    fn ctrl(m: &Ctrl) -> Bytes {
        Frame::ctrl(m).unwrap().to_bytes().unwrap()
    }

    fn run_start() -> RunStart {
        RunStart {
            run_id: "j".into(),
            repo_url: "https://github.com/a/b".into(),
            base_ref: "main".into(),
            branch: "x".into(),
            push: false,
            clone_token: "t".into(),
            prompt: "p".into(),
            timeout_secs: 1,
            callback_url: "http://x".into(),
            callback_token: "t".into(),
            log_channel: None,
        }
    }

    #[test]
    fn allows_pty_traffic() {
        let open = Ctrl::Open {
            channel: 1,
            kind: ChannelKind::Pty,
            cols: 80,
            rows: 24,
            command: None,
        };
        assert!(check_browser_message(&ctrl(&open)).is_ok());
        let resize = Ctrl::Resize {
            channel: 1,
            cols: 1,
            rows: 1,
        };
        assert!(check_browser_message(&ctrl(&resize)).is_ok());
        for f in [
            Frame::data_bin(1, &b"ls\n"[..]),
            Frame::window(1, 100),
            Frame::close(1, 0, ""),
        ] {
            assert!(check_browser_message(&f.to_bytes().unwrap()).is_ok());
        }
    }

    #[test]
    fn rejects_run_control() {
        for m in [
            Ctrl::RunStart(run_start()),
            Ctrl::RunCancel { run_id: "j".into() },
            Ctrl::RunStatus { run_id: "j".into() },
            Ctrl::RunState {
                run_id: "j".into(),
                state: RunPhase::Running,
                exit_code: None,
                summary: None,
            },
            Ctrl::Hello {
                versions: vec![1],
                client: enzarb_proto::ClientKind::Executor,
            },
        ] {
            assert!(matches!(
                check_browser_message(&ctrl(&m)),
                Err(Reject::ForbiddenCtrl(_))
            ));
        }
    }

    #[test]
    fn rejects_malformed_and_smuggled() {
        // Truncated, trailing bytes, bad type, non-JSON CTRL, unknown CTRL type.
        let good = ctrl(&Ctrl::Resize {
            channel: 1,
            cols: 1,
            rows: 1,
        });
        assert!(check_browser_message(&good.slice(..5)).is_err());
        let mut trailing = good.to_vec();
        trailing.push(0);
        assert!(check_browser_message(&Bytes::from(trailing)).is_err());
        assert!(
            check_browser_message(&Bytes::from_static(&[9, 0, 0, 0, 0, 0, 0, 0, 0, 0])).is_err()
        );
        let bad = Frame::new(FrameType::Ctrl, 0, &b"not json"[..]);
        assert!(check_browser_message(&bad.to_bytes().unwrap()).is_err());
        let unk = Frame::new(FrameType::Ctrl, 0, &br#"{"type":"future_thing"}"#[..]);
        assert!(check_browser_message(&unk.to_bytes().unwrap()).is_err());
        let partial = Frame::new(FrameType::Ctrl, 0, &br#"{"type":"run_start"}"#[..]);
        assert!(check_browser_message(&partial.to_bytes().unwrap()).is_err());
        // CTRL on a non-zero channel is invalid.
        let mut b = good.to_vec();
        b[5] = 7;
        assert!(check_browser_message(&Bytes::from(b)).is_err());
    }
}
