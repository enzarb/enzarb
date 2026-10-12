//! Security boundary: what a browser may send towards the sandbox.
//!
//! Only the workflows executor may start, cancel or poll runs. The proxy
//! therefore fully decodes every browser frame and applies an allowlist to CTRL
//! messages. Anything unparseable is rejected too (fail closed), so there is no
//! way to smuggle a `run_*` message past the proxy with a malformed envelope
//! that the sandbox would still interpret.
//!
//! # `open` policy (chat tab, ADR-0014)
//!
//! `open` is the one CTRL message judged by content, and a refused `open` is
//! answered, not fatal, so a refusal never costs the terminal on the same
//! socket:
//!
//! * `kind: pty` is forwarded as before.
//! * `kind: acp` is forwarded only if `acp` was negotiated in this socket's
//!   `welcome` (the sandbox advertised it AND this proxy knows it), there is no
//!   other live `acp` channel on this socket, and `repo`/`resume` are within
//!   the protocol limits. Otherwise the proxy itself answers `open_error`
//!   (`unsupported_kind`, `chat_limit`, `invalid_request`) and nothing reaches
//!   the sandbox.
//! * Any other `kind` decodes to `ChannelKind::Unknown` and is answered
//!   `unsupported_kind` by the proxy. An unknown kind is never forwarded: the
//!   proxy cannot know what a future sandbox would do with it.
//!
//! Frames that pass are forwarded as the **original bytes**; the proxy never
//! re-encodes a message (an `Unknown` kind would serialize as `"unknown"`).

use bytes::Bytes;
use enzarb_proto::{
    ChannelKind, Ctrl, Frame, FrameType, MAX_REPO_LEN, MAX_RESUME_LEN, feature, open_error_code,
};

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

/// What to do with a valid browser frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Send the original bytes to the sandbox.
    Forward(Allowed),
    /// Do not forward; send these bytes (an `open_error`) back to the browser.
    Reply(Bytes),
}

/// Features this proxy knows how to validate and relay. `welcome.features` to
/// the browser is the sandbox's list intersected with this one.
pub const PROXY_FEATURES: &[&str] = &[feature::ACP];

/// The features to advertise to the browser.
pub fn negotiated_features(sandbox: &[String]) -> Vec<String> {
    PROXY_FEATURES
        .iter()
        .filter(|f| sandbox.iter().any(|s| s == *f))
        .map(|f| (*f).to_string())
        .collect()
}

/// Per-socket state for the browser->sandbox direction.
#[derive(Debug, Default)]
pub struct Guard {
    acp_enabled: bool,
    /// The one `acp` channel this socket has open (or is opening).
    acp_channel: Option<u32>,
}

fn open_error(channel: u32, code: &str, message: &str) -> Result<Verdict, Reject> {
    let m = Ctrl::OpenError {
        channel,
        message: message.into(),
        code: Some(code.into()),
    };
    let b = Frame::ctrl(&m)
        .and_then(|f| f.to_bytes())
        .map_err(|e| Reject::Malformed(e.code().to_string()))?;
    Ok(Verdict::Reply(b))
}

impl Guard {
    /// `features` is what was advertised to this browser in `welcome`.
    pub fn new(features: &[String]) -> Self {
        Self {
            acp_enabled: features.iter().any(|f| f == feature::ACP),
            acp_channel: None,
        }
    }

    /// Validate one browser->sandbox websocket binary message (exactly one frame).
    pub fn check_browser_message(&mut self, msg: &Bytes) -> Result<Verdict, Reject> {
        let frame =
            Frame::from_bytes(msg.clone()).map_err(|e| Reject::Malformed(e.code().to_string()))?;
        let allowed = Allowed {
            frame_type: frame.frame_type,
            channel: frame.channel,
            payload_len: frame.payload.len(),
        };
        match frame.frame_type {
            FrameType::Ctrl => {
                let ctrl = frame
                    .ctrl_message()
                    .map_err(|e| Reject::Malformed(e.code().to_string()))?;
                ctrl_allowed(&ctrl).map_err(Reject::ForbiddenCtrl)?;
                if let Ctrl::Open {
                    channel,
                    kind,
                    repo,
                    resume,
                    ..
                } = &ctrl
                {
                    let channel = *channel;
                    if channel == 0 {
                        return Err(Reject::Malformed("open on channel 0".into()));
                    }
                    if repo.as_ref().is_some_and(|r| r.len() > MAX_REPO_LEN)
                        || resume.as_ref().is_some_and(|r| r.len() > MAX_RESUME_LEN)
                    {
                        return open_error(
                            channel,
                            open_error_code::INVALID_REQUEST,
                            "repo or resume too long",
                        );
                    }
                    if self.acp_channel == Some(channel) {
                        return open_error(
                            channel,
                            open_error_code::INVALID_REQUEST,
                            "channel already in use",
                        );
                    }
                    match kind {
                        ChannelKind::Pty => {}
                        ChannelKind::Acp => {
                            if !self.acp_enabled {
                                return open_error(
                                    channel,
                                    open_error_code::UNSUPPORTED_KIND,
                                    "acp channels are not available",
                                );
                            }
                            if self.acp_channel.is_some() {
                                return open_error(
                                    channel,
                                    open_error_code::CHAT_LIMIT,
                                    "one chat per connection",
                                );
                            }
                            self.acp_channel = Some(channel);
                        }
                        ChannelKind::Unknown => {
                            return open_error(
                                channel,
                                open_error_code::UNSUPPORTED_KIND,
                                "unsupported channel kind",
                            );
                        }
                    }
                }
            }
            FrameType::Close if self.acp_channel == Some(frame.channel) => {
                self.acp_channel = None;
            }
            _ => {}
        }
        Ok(Verdict::Forward(allowed))
    }

    /// Observe a sandbox->browser frame (never altered) to track the `acp`
    /// slot: the sandbox closing the channel or refusing its `open` frees it.
    pub fn observe_sandbox_message(&mut self, msg: &Bytes) {
        let Some(slot) = self.acp_channel else { return };
        let Ok(frame) = Frame::from_bytes(msg.clone()) else {
            return;
        };
        match frame.frame_type {
            FrameType::Close if frame.channel == slot => self.acp_channel = None,
            FrameType::Ctrl => {
                if let Ok(Ctrl::OpenError { channel, .. }) = frame.ctrl_message()
                    && channel == slot
                {
                    self.acp_channel = None;
                }
            }
            _ => {}
        }
    }
}

/// CTRL messages a browser (`client: "ui"`) may send after the handshake.
fn ctrl_allowed(m: &Ctrl) -> Result<(), &'static str> {
    match m {
        Ctrl::Open { .. } | Ctrl::Resize { .. } | Ctrl::Error { .. } | Ctrl::CredentialStatus => {
            Ok(())
        }
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
        Ctrl::CredentialState { .. } => Err("credential_state"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_proto::{ChannelKind, RunPhase, RunStart};

    fn check(m: &Bytes) -> Result<Verdict, Reject> {
        Guard::new(&["acp".to_string()]).check_browser_message(m)
    }

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
            deps_token: None,
            npm_token: None,
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
            repo: None,
            resume: None,
        };
        assert!(check(&ctrl(&open)).is_ok());
        let resize = Ctrl::Resize {
            channel: 1,
            cols: 1,
            rows: 1,
        };
        assert!(check(&ctrl(&resize)).is_ok());
        assert!(check(&ctrl(&Ctrl::CredentialStatus)).is_ok());
        for f in [
            Frame::data_bin(1, &b"ls\n"[..]),
            Frame::window(1, 100),
            Frame::close(1, 0, ""),
        ] {
            assert!(check(&f.to_bytes().unwrap()).is_ok());
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
            assert!(matches!(check(&ctrl(&m)), Err(Reject::ForbiddenCtrl(_))));
        }
    }

    #[test]
    fn rejects_credential_state_from_browser() {
        let m = Ctrl::CredentialState {
            kind: enzarb_proto::CredentialKind::ClaudeOauth,
            configured: true,
        };
        assert!(matches!(
            check(&ctrl(&m)),
            Err(Reject::ForbiddenCtrl("credential_state"))
        ));
    }

    #[test]
    fn rejects_malformed_and_smuggled() {
        // Truncated, trailing bytes, bad type, non-JSON CTRL, unknown CTRL type.
        let good = ctrl(&Ctrl::Resize {
            channel: 1,
            cols: 1,
            rows: 1,
        });
        assert!(check(&good.slice(..5)).is_err());
        let mut trailing = good.to_vec();
        trailing.push(0);
        assert!(check(&Bytes::from(trailing)).is_err());
        assert!(check(&Bytes::from_static(&[9, 0, 0, 0, 0, 0, 0, 0, 0, 0])).is_err());
        let bad = Frame::new(FrameType::Ctrl, 0, &b"not json"[..]);
        assert!(check(&bad.to_bytes().unwrap()).is_err());
        let unk = Frame::new(FrameType::Ctrl, 0, &br#"{"type":"future_thing"}"#[..]);
        assert!(check(&unk.to_bytes().unwrap()).is_err());
        let partial = Frame::new(FrameType::Ctrl, 0, &br#"{"type":"run_start"}"#[..]);
        assert!(check(&partial.to_bytes().unwrap()).is_err());
        // CTRL on a non-zero channel is invalid.
        let mut b = good.to_vec();
        b[5] = 7;
        assert!(check(&Bytes::from(b)).is_err());
    }

    fn open(
        channel: u32,
        kind: ChannelKind,
        repo: Option<String>,
        resume: Option<String>,
    ) -> Bytes {
        ctrl(&Ctrl::Open {
            channel,
            kind,
            cols: 0,
            rows: 0,
            command: None,
            repo,
            resume,
        })
    }

    fn code_of(v: Verdict) -> (u32, Option<String>) {
        let Verdict::Reply(b) = v else {
            panic!("expected reply, got {v:?}")
        };
        match Frame::from_bytes(b).unwrap().ctrl_message().unwrap() {
            Ctrl::OpenError { channel, code, .. } => (channel, code),
            m => panic!("{m:?}"),
        }
    }

    fn acp_guard() -> Guard {
        Guard::new(&["acp".to_string()])
    }

    #[test]
    fn acp_open_is_forwarded() {
        let mut g = acp_guard();
        let m = open(
            3,
            ChannelKind::Acp,
            Some("https://github.com/a/b".into()),
            Some("sess".into()),
        );
        assert!(matches!(
            g.check_browser_message(&m),
            Ok(Verdict::Forward(_))
        ));
        // DATA on the channel is allowed.
        let d = Frame::data_bin(3, &b"{}\n"[..]).to_bytes().unwrap();
        assert!(matches!(
            g.check_browser_message(&d),
            Ok(Verdict::Forward(_))
        ));
    }

    #[test]
    fn acp_open_without_feature_is_unsupported() {
        for feats in [vec![], vec!["other".to_string()]] {
            let mut g = Guard::new(&feats);
            let v = g
                .check_browser_message(&open(3, ChannelKind::Acp, None, None))
                .unwrap();
            assert_eq!(code_of(v), (3, Some("unsupported_kind".into())));
        }
    }

    #[test]
    fn unknown_kind_is_answered_not_forwarded() {
        let raw = br#"{"type":"open","channel":4,"kind":"future","cols":0,"rows":0}"#;
        let m = Frame::new(FrameType::Ctrl, 0, &raw[..]).to_bytes().unwrap();
        for feats in [vec![], vec!["acp".to_string()]] {
            let v = Guard::new(&feats).check_browser_message(&m).unwrap();
            assert_eq!(code_of(v), (4, Some("unsupported_kind".into())));
        }
    }

    #[test]
    fn second_acp_open_is_chat_limit_until_closed() {
        let mut g = acp_guard();
        assert!(matches!(
            g.check_browser_message(&open(3, ChannelKind::Acp, None, None)),
            Ok(Verdict::Forward(_))
        ));
        let v = g
            .check_browser_message(&open(5, ChannelKind::Acp, None, None))
            .unwrap();
        assert_eq!(code_of(v), (5, Some("chat_limit".into())));
        // The live channel's id cannot be reused for a pty either.
        let v = g
            .check_browser_message(&open(3, ChannelKind::Pty, None, None))
            .unwrap();
        assert_eq!(code_of(v), (3, Some("invalid_request".into())));
        // A pty alongside is fine.
        assert!(matches!(
            g.check_browser_message(&open(7, ChannelKind::Pty, None, None)),
            Ok(Verdict::Forward(_))
        ));
        // Browser closes it: a new one is allowed.
        let c = Frame::close(3, 0, "").to_bytes().unwrap();
        assert!(matches!(
            g.check_browser_message(&c),
            Ok(Verdict::Forward(_))
        ));
        assert!(matches!(
            g.check_browser_message(&open(5, ChannelKind::Acp, None, None)),
            Ok(Verdict::Forward(_))
        ));
    }

    #[test]
    fn sandbox_close_or_refusal_frees_the_slot() {
        let refused = Frame::ctrl(&Ctrl::OpenError {
            channel: 3,
            message: "no".into(),
            code: Some("chat_disabled".into()),
        })
        .unwrap()
        .to_bytes()
        .unwrap();
        let closed = Frame::close(3, 0, "idle").to_bytes().unwrap();
        let other = Frame::close(9, 0, "idle").to_bytes().unwrap();
        for (frame, frees) in [(refused, true), (closed, true), (other, false)] {
            let mut g = acp_guard();
            g.check_browser_message(&open(3, ChannelKind::Acp, None, None))
                .unwrap();
            g.observe_sandbox_message(&frame);
            let v = g
                .check_browser_message(&open(5, ChannelKind::Acp, None, None))
                .unwrap();
            assert_eq!(matches!(v, Verdict::Forward(_)), frees);
        }
    }

    #[test]
    fn repo_and_resume_limits() {
        let ok_repo = "r".repeat(enzarb_proto::MAX_REPO_LEN);
        let ok_resume = "s".repeat(enzarb_proto::MAX_RESUME_LEN);
        let mut g = acp_guard();
        assert!(matches!(
            g.check_browser_message(&open(
                3,
                ChannelKind::Acp,
                Some(ok_repo.clone()),
                Some(ok_resume.clone())
            )),
            Ok(Verdict::Forward(_))
        ));
        for (repo, resume) in [
            (Some(format!("{ok_repo}x")), None),
            (None, Some(format!("{ok_resume}x"))),
        ] {
            let mut g = acp_guard();
            let v = g
                .check_browser_message(&open(3, ChannelKind::Acp, repo.clone(), resume.clone()))
                .unwrap();
            assert_eq!(code_of(v), (3, Some("invalid_request".into())));
            // Applies to pty too, and a refused open does not take the slot.
            let v = g
                .check_browser_message(&open(4, ChannelKind::Pty, repo, resume))
                .unwrap();
            assert_eq!(code_of(v), (4, Some("invalid_request".into())));
            assert!(matches!(
                g.check_browser_message(&open(5, ChannelKind::Acp, None, None)),
                Ok(Verdict::Forward(_))
            ));
        }
    }

    #[test]
    fn open_on_channel_zero_is_rejected() {
        assert!(matches!(
            acp_guard().check_browser_message(&open(0, ChannelKind::Acp, None, None)),
            Err(Reject::Malformed(_))
        ));
    }

    #[test]
    fn features_are_intersected() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(negotiated_features(&s(&["acp", "future"])), s(&["acp"]));
        assert!(negotiated_features(&s(&["future"])).is_empty());
        assert!(negotiated_features(&[]).is_empty());
    }
}
