//! Claude OAuth token capture from `claude setup-token` PTYs.
//!
//! The observer strips ANSI sequences (state carried across chunks), looks for
//! the `sk-ant-oat01-` prefix and collects the token, joining it across the
//! line wraps the terminal UI inserts. The token is written atomically with
//! mode 0600 and the client is told with `credential_captured`.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use enzarb_proto::session::Session;
use enzarb_proto::{CredentialKind, Ctrl};

use crate::observer::{PtyInfo, PtyObserver, PtyObserverFactory};

const PREFIX: &[u8] = b"sk-ant-oat01-";
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
    std::fs::create_dir_all(dir)?;
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
        .and_then(|()| std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)))
        .and_then(|()| std::fs::rename(&tmp, path));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Write `token` to `path` atomically with mode 0600.
pub fn write_token(path: &Path, token: &str) -> std::io::Result<()> {
    write_atomic(path, token.as_bytes(), 0o600)
}

/// Observes PTYs whose command contains `setup-token`.
pub struct ClaudeTokenCapture {
    token_file: PathBuf,
}

impl ClaudeTokenCapture {
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
    fn store(&self, token: String) {
        match write_token(&self.token_file, &token) {
            Ok(()) => {
                tracing::info!("claude oauth token captured");
                if let Err(e) = self.session.send_ctrl(&Ctrl::CredentialCaptured {
                    kind: CredentialKind::ClaudeOauth,
                }) {
                    tracing::warn!(error = %e, "credential_captured not sent");
                }
            }
            Err(e) => tracing::error!(error = %e, "cannot store claude token"),
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
    fn writes_private_file_atomically() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/claude-token");
        write_token(&p, "one").unwrap();
        write_token(&p, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_dir(d.path().join("sub")).unwrap().count(), 1);
    }
}
