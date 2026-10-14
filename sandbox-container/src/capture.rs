//! Claude OAuth token capture from `claude setup-token` PTYs.
//!
//! The observer strips ANSI sequences (state carried across chunks), looks for
//! the `sk-ant-oat01-` prefix and collects the token, joining it across the
//! line wraps the terminal UI inserts.
//!
//! Every token-bearing frame is gated on the client offering
//! [`feature::CLAUDE_USER_TOKEN`] in `hello` (only a proxy that stores the
//! token and never relays it to the browser does; ADR-0004 amendment
//! 2026-10-13). With it, a capture goes out as `credential_captured{token}`
//! and nothing is written to disk, and a legacy file is promoted (see
//! [`LegacyPromotion`]). Without it (older proxies, which relay frames
//! verbatim), behaviour is unchanged: the token is written atomically with mode
//! 0600 to the legacy file and the client gets a token-less
//! `credential_captured`. The feature, like the client kind, is self-declared
//! in `hello`; the only authentication is the shared bearer token.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use enzarb_proto::session::Session;
use enzarb_proto::{ClientKind, CredentialKind, Ctrl, Secret, feature};

use crate::observer::{PtyInfo, PtyObserver, PtyObserverFactory};

const PREFIX: &[u8] = b"sk-ant-oat01-";
/// The prefix every Claude OAuth token starts with.
pub const TOKEN_PREFIX: &str = "sk-ant-oat01-";
/// A token shorter than this followed by a line break is assumed to be wrapped.
const MIN_LEN: usize = 100;
/// Upper bound while collecting; guards against runaway output.
const MAX_LEN: usize = 1024;

/// Strips ANSI escape sequences from a byte stream, across chunk boundaries.
#[derive(Default)]
pub struct AnsiStripper {
    state: AnsiState,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum AnsiState {
    #[default]
    Text,
    Esc,
    /// `ESC (`, `ESC )` ...: one more byte follows.
    Skip1,
    Csi,
    Osc,
    OscEsc,
}

impl AnsiStripper {
    /// Feed bytes; plain bytes are appended to `out`.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        use AnsiState::*;
        for &b in chunk {
            self.state = match (self.state, b) {
                (Text, 0x1b) => Esc,
                (Text, _) => {
                    out.push(b);
                    Text
                }
                (Esc, b'[') => Csi,
                (Esc, b']') => Osc,
                (Esc, b'(' | b')' | b'*' | b'+' | b'#') => Skip1,
                (Esc, _) => Text,
                (Skip1, _) => Text,
                (Csi, 0x40..=0x7e) => Text,
                (Csi, _) => Csi,
                (Osc, 0x07) => Text,
                (Osc, 0x1b) => OscEsc,
                (Osc, _) => Osc,
                (OscEsc, b'\\') => Text,
                (OscEsc, _) => Osc,
            };
        }
    }
}

enum Scan {
    /// Looking for the prefix; `window` holds the last bytes seen.
    Prefix {
        window: Vec<u8>,
    },
    Token {
        buf: Vec<u8>,
    },
    Done,
}

/// Finds the token in a plain-text byte stream.
pub struct TokenScanner {
    scan: Scan,
}

impl Default for TokenScanner {
    fn default() -> Self {
        Self {
            scan: Scan::Prefix { window: Vec::new() },
        }
    }
}

fn token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

impl TokenScanner {
    /// Feed plain bytes; returns the token once complete.
    pub fn push(&mut self, plain: &[u8]) -> Option<String> {
        for &b in plain {
            match &mut self.scan {
                Scan::Done => return None,
                Scan::Prefix { window } => {
                    if b == b'\r' || b == b'\n' {
                        continue;
                    }
                    window.push(b);
                    if window.len() > PREFIX.len() {
                        window.remove(0);
                    }
                    if window.as_slice() == PREFIX {
                        self.scan = Scan::Token {
                            buf: PREFIX.to_vec(),
                        };
                    }
                }
                Scan::Token { buf } => {
                    if token_byte(b) {
                        buf.push(b);
                        if buf.len() >= MAX_LEN {
                            self.scan = Scan::Prefix { window: Vec::new() };
                        }
                    } else if (b == b'\r' || b == b'\n') && buf.len() < MIN_LEN {
                        // Probably a wrapped line; keep collecting.
                    } else {
                        return self.finish();
                    }
                }
            }
        }
        None
    }

    /// The stream ended; a long enough pending token counts.
    pub fn finish(&mut self) -> Option<String> {
        let Scan::Token { buf } = std::mem::replace(&mut self.scan, Scan::Done) else {
            return None;
        };
        (buf.len() >= MIN_LEN).then(|| String::from_utf8_lossy(&buf).into_owned())
    }
}

/// Write `data` to `path` atomically (temp file + rename) with `mode`.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    crate::privs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp{}-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)?;
    let res = f
        .write_all(data)
        .and_then(|()| f.sync_all())
        .and_then(|()| crate::privs::own(&tmp))
        .and_then(|()| std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)))
        .and_then(|()| std::fs::rename(&tmp, path));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Write `token` to `path` atomically with mode 0600 (legacy peers only).
pub fn write_token(path: &Path, token: &str) -> std::io::Result<()> {
    write_atomic(path, token.as_bytes(), 0o600)
}

/// Whether the client may receive Claude tokens: a proxy that offered
/// [`feature::CLAUDE_USER_TOKEN`] in `hello`.
pub fn user_token_peer(session: &Session) -> bool {
    session.peer_kind() == Some(ClientKind::Proxy)
        && session.peer_has_feature(feature::CLAUDE_USER_TOKEN)
}

/// The legacy token file's contents, trimmed, if non-empty.
pub fn read_legacy(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The legacy token file's contents if they look like an OAuth token.
pub fn read_legacy_valid(path: &Path) -> Option<String> {
    read_legacy(path).filter(|t| t.starts_with(TOKEN_PREFIX))
}

/// The Claude token for one run or chat: the per-request token
/// (`run_start.claude_token` / `open.claude_token`) when present and
/// non-empty, otherwise the legacy file (old proxies and workflows), read with
/// `legacy`. `None` means `no_credential`. The token is never written to disk.
pub fn resolve_token(
    per_request: Option<&Secret>,
    legacy: impl FnOnce() -> Option<String>,
) -> Option<String> {
    match per_request.map(|s| s.expose().trim()) {
        Some(t) if !t.is_empty() => Some(t.to_string()),
        _ => legacy(),
    }
}

/// Per-session promotion of the legacy token file to the proxy:
/// `credential_captured{token, legacy: true}` on connect, and the file is
/// deleted only when the proxy acks with `credential_state{configured: true}`
/// and the file still holds the promoted token. A peer that never acks (an old
/// proxy) leaves the file in place, so it keeps working as the fallback.
#[derive(Default)]
pub struct LegacyPromotion {
    sent: Option<Secret>,
}

impl std::fmt::Debug for LegacyPromotion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyPromotion")
            .field("sent", &self.sent.is_some())
            .finish()
    }
}

impl LegacyPromotion {
    /// Send the legacy token once per session, to proxies only (the frame is
    /// proxy-only: the proxy stores it and never forwards it).
    pub fn promote(&mut self, session: &Session, legacy_file: &Path) {
        if self.sent.is_some() || !user_token_peer(session) {
            return;
        }
        let Some(token) = read_legacy_valid(legacy_file) else {
            return;
        };
        let token = Secret::new(token);
        match session.send_ctrl(&Ctrl::CredentialCaptured {
            kind: CredentialKind::ClaudeOauth,
            token: Some(token.clone()),
            legacy: Some(true),
        }) {
            Ok(()) => {
                tracing::info!("legacy claude token promoted; awaiting ack");
                self.sent = Some(token);
            }
            Err(e) => tracing::warn!(error = %e, "legacy credential_captured not sent"),
        }
    }

    /// Handle a client `credential_state`: on `configured: true` after a
    /// promotion, delete the legacy file if it still holds the promoted token.
    /// Returns true when the file was removed.
    pub fn ack(&mut self, configured: bool, legacy_file: &Path) -> bool {
        if !configured {
            return false;
        }
        let Some(sent) = self.sent.take() else {
            return false;
        };
        if read_legacy(legacy_file).as_deref() != Some(sent.expose()) {
            tracing::info!("legacy claude token changed since promotion; kept");
            return false;
        }
        match std::fs::remove_file(legacy_file) {
            Ok(()) => {
                tracing::info!("legacy claude token acked by proxy; file removed");
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot remove legacy claude token");
                false
            }
        }
    }
}

/// Observes PTYs whose command contains `setup-token`.
pub struct ClaudeTokenCapture {
    token_file: PathBuf,
}

impl ClaudeTokenCapture {
    /// `token_file` is the legacy file, written only for legacy peers.
    pub fn new(token_file: PathBuf) -> Self {
        Self { token_file }
    }
}

impl PtyObserverFactory for ClaudeTokenCapture {
    fn observe(&self, pty: &PtyInfo) -> Option<Box<dyn PtyObserver>> {
        let cmd = pty.command.as_ref()?;
        if !cmd.iter().any(|a| a.contains("setup-token")) {
            return None;
        }
        Some(Box::new(CaptureObserver {
            ansi: AnsiStripper::default(),
            scanner: TokenScanner::default(),
            token_file: self.token_file.clone(),
            session: pty.session.clone(),
            plain: Vec::new(),
        }))
    }
}

struct CaptureObserver {
    ansi: AnsiStripper,
    scanner: TokenScanner,
    token_file: PathBuf,
    session: Session,
    plain: Vec<u8>,
}

impl CaptureObserver {
    /// A user-token peer gets the token in the frame and nothing is kept on
    /// disk; any other peer gets the old behaviour (file + token-less frame).
    fn store(&self, token: String) {
        let frame = if user_token_peer(&self.session) {
            Ctrl::CredentialCaptured {
                kind: CredentialKind::ClaudeOauth,
                token: Some(Secret::new(token)),
                legacy: None,
            }
        } else {
            if let Err(e) = write_token(&self.token_file, &token) {
                tracing::error!(error = %e, "cannot store claude token");
                return;
            }
            Ctrl::CredentialCaptured {
                kind: CredentialKind::ClaudeOauth,
                token: None,
                legacy: None,
            }
        };
        match self.session.send_ctrl(&frame) {
            Ok(()) => tracing::info!("claude oauth token captured"),
            Err(e) => tracing::warn!(error = %e, "credential_captured not sent"),
        }
    }
}

impl PtyObserver for CaptureObserver {
    fn on_output(&mut self, chunk: &[u8]) {
        self.plain.clear();
        self.ansi.push(chunk, &mut self.plain);
        if let Some(t) = self.scanner.push(&self.plain) {
            self.store(t);
        }
    }

    fn on_close(&mut self) {
        if let Some(t) = self.scanner.finish() {
            self.store(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> String {
        format!("sk-ant-oat01-{}", "aB3_-".repeat(20))
    }

    fn scan(chunks: &[&[u8]]) -> Option<String> {
        let (mut a, mut s) = (AnsiStripper::default(), TokenScanner::default());
        let mut out = None;
        for c in chunks {
            let mut plain = Vec::new();
            a.push(c, &mut plain);
            out = out.or(s.push(&plain));
        }
        out.or_else(|| s.finish())
    }

    #[test]
    fn strips_ansi_across_chunks() {
        let mut a = AnsiStripper::default();
        let mut out = Vec::new();
        a.push(b"ab\x1b[3", &mut out);
        a.push(b"1;1mcd\x1b]0;title\x07e\x1b(Bf", &mut out);
        assert_eq!(out, b"abcdef");
    }

    #[test]
    fn finds_token_split_everywhere() {
        let t = token();
        let text = format!("\x1b[1mYour token:\x1b[0m\r\n{t}\r\nStore it safely\r\n");
        for cut in 1..text.len() {
            let (a, b) = text.as_bytes().split_at(cut);
            assert_eq!(scan(&[a, b]).as_deref(), Some(t.as_str()), "cut {cut}");
        }
    }

    #[test]
    fn joins_wrapped_token() {
        let t = token();
        let (a, b) = t.split_at(60);
        let text = format!("x\r\n{a}\r\n{b}\r\n\r\nUse this");
        assert_eq!(scan(&[text.as_bytes()]).as_deref(), Some(t.as_str()));
    }

    #[test]
    fn token_at_end_of_stream_and_noise() {
        let t = token();
        assert_eq!(scan(&[t.as_bytes()]).as_deref(), Some(t.as_str()));
        assert_eq!(scan(&[b"sk-ant-oat01-short\n"]), None);
        assert_eq!(scan(&[b"nothing here"]), None);
    }

    #[test]
    fn resolve_prefers_the_request_token() {
        let req = Secret::new("  sk-ant-oat01-req \n");
        let legacy = || Some("sk-ant-oat01-file".to_string());
        assert_eq!(
            resolve_token(Some(&req), legacy).as_deref(),
            Some("sk-ant-oat01-req")
        );
        assert_eq!(
            resolve_token(Some(&Secret::new(" \n")), legacy).as_deref(),
            Some("sk-ant-oat01-file")
        );
        assert_eq!(
            resolve_token(None, legacy).as_deref(),
            Some("sk-ant-oat01-file")
        );
        assert_eq!(resolve_token(None, || None), None);
    }

    #[test]
    fn legacy_reads_validate() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("claude-token");
        assert_eq!(read_legacy(&p), None);
        std::fs::write(&p, " \n").unwrap();
        assert_eq!(read_legacy(&p), None);
        std::fs::write(&p, "wrong\n").unwrap();
        assert_eq!(read_legacy(&p).as_deref(), Some("wrong"));
        assert_eq!(read_legacy_valid(&p), None);
        std::fs::write(&p, "sk-ant-oat01-x\n").unwrap();
        assert_eq!(read_legacy_valid(&p).as_deref(), Some("sk-ant-oat01-x"));
    }

    #[test]
    fn ack_without_promotion_or_unconfigured_keeps_the_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("claude-token");
        std::fs::write(&p, "sk-ant-oat01-x").unwrap();
        let mut lp = LegacyPromotion::default();
        assert!(!lp.ack(true, &p));
        lp.sent = Some(Secret::new("sk-ant-oat01-x"));
        assert!(!lp.ack(false, &p));
        assert!(p.exists());
        assert!(!format!("{lp:?}").contains("oat01"));
        assert!(lp.ack(true, &p));
        assert!(!p.exists());
    }

    #[test]
    fn writes_private_file_atomically() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/claude-token");
        write_atomic(&p, b"one", 0o600).unwrap();
        write_atomic(&p, b"two", 0o600).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_dir(d.path().join("sub")).unwrap().count(), 1);
    }
}
