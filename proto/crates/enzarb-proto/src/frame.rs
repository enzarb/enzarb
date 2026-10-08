//! Frame layout and codec.
//!
//! ```text
//! 0      1      2             6              10
//! +------+------+-------------+--------------+----------------+
//! | type |flags | channel u32 | length u32   | payload[length]|
//! +------+------+-------------+--------------+----------------+
//! ```
//! All integers are big-endian.

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::ctrl::Ctrl;

/// Header size in bytes.
pub const HEADER_LEN: usize = 10;
/// Maximum payload length of any frame (1 MiB).
pub const MAX_PAYLOAD_LEN: usize = 1 << 20;
/// Maximum credit a sender may hold for a channel (and maximum single grant).
pub const MAX_CREDIT: u32 = i32::MAX as u32;
/// Initial per-channel, per-direction window in bytes (256 KiB).
pub const INITIAL_WINDOW: u32 = 256 * 1024;

/// Well-known `CLOSE` codes. Others may be used; unknown codes are opaque.
pub mod close_code {
    pub const NORMAL: u16 = 0;
    pub const PROTOCOL_ERROR: u16 = 1;
    pub const GOING_AWAY: u16 = 2;
    pub const CANCELLED: u16 = 3;
}

/// Frame type byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameType {
    Ctrl = 1,
    DataBin = 2,
    DataJson = 3,
    Window = 4,
    Close = 5,
}

impl FrameType {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Ctrl,
            2 => Self::DataBin,
            3 => Self::DataJson,
            4 => Self::Window,
            5 => Self::Close,
            _ => return None,
        })
    }

    /// Name used in the spec and the shared test vectors.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ctrl => "CTRL",
            Self::DataBin => "DATA_BIN",
            Self::DataJson => "DATA_JSON",
            Self::Window => "WINDOW",
            Self::Close => "CLOSE",
        }
    }
}

/// Codec errors. `code()` returns a stable identifier shared with the vectors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("truncated frame")]
    Truncated,
    #[error("{0} trailing bytes after frame")]
    TrailingBytes(usize),
    #[error("unknown frame type {0}")]
    UnknownType(u8),
    #[error("non-zero flags {0:#04x}")]
    UnknownFlags(u8),
    #[error("payload of {0} bytes exceeds the maximum")]
    PayloadTooLarge(usize),
    #[error("CTRL frames must use channel 0")]
    CtrlChannel,
    #[error("{0} frames must not use channel 0")]
    ZeroChannel(&'static str),
    #[error("empty data frame")]
    EmptyData,
    #[error("WINDOW payload must be 4 bytes")]
    BadWindowPayload,
    #[error("WINDOW credit must be in 1..=2^31-1")]
    BadCredit,
    #[error("malformed CLOSE payload")]
    BadClosePayload,
    #[error("json: {0}")]
    Json(String),
}

impl FrameError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Truncated => "truncated",
            Self::TrailingBytes(_) => "trailing_bytes",
            Self::UnknownType(_) => "unknown_type",
            Self::UnknownFlags(_) => "unknown_flags",
            Self::PayloadTooLarge(_) => "payload_too_large",
            Self::CtrlChannel => "ctrl_channel",
            Self::ZeroChannel(_) => "zero_channel",
            Self::EmptyData => "empty_data",
            Self::BadWindowPayload => "bad_window_payload",
            Self::BadCredit => "bad_credit",
            Self::BadClosePayload => "bad_close_payload",
            Self::Json(_) => "json",
        }
    }
}

/// Decoded `CLOSE` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseInfo {
    pub code: u16,
    pub reason: String,
}

/// One protocol frame. The payload is a cheap-to-clone `Bytes` slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_type: FrameType,
    pub flags: u8,
    pub channel: u32,
    pub payload: Bytes,
}

/// A parsed and validated 10-byte frame header.
///
/// Lets forwarders (e.g. sandbox-proxy) route a websocket message by type and
/// channel without decoding or copying the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub frame_type: FrameType,
    pub flags: u8,
    pub channel: u32,
    /// Payload length in bytes (already checked against [`MAX_PAYLOAD_LEN`]).
    pub len: usize,
}

impl FrameHeader {
    /// Parse the header at the start of `b` (which may contain more bytes).
    /// Applies the same checks as the full decoder: type, flags, length cap
    /// and the channel rules.
    pub fn parse(b: &[u8]) -> Result<Self, FrameError> {
        if b.len() < HEADER_LEN {
            return Err(FrameError::Truncated);
        }
        parse_header(b)
    }
}

fn parse_header(b: &[u8]) -> Result<FrameHeader, FrameError> {
    debug_assert!(b.len() >= HEADER_LEN);
    let frame_type = FrameType::from_u8(b[0]).ok_or(FrameError::UnknownType(b[0]))?;
    let flags = b[1];
    if flags != 0 {
        return Err(FrameError::UnknownFlags(flags));
    }
    let channel = u32::from_be_bytes([b[2], b[3], b[4], b[5]]);
    let len = u32::from_be_bytes([b[6], b[7], b[8], b[9]]) as usize;
    if len > MAX_PAYLOAD_LEN {
        return Err(FrameError::PayloadTooLarge(len));
    }
    check_channel(frame_type, channel)?;
    Ok(FrameHeader {
        frame_type,
        flags,
        channel,
        len,
    })
}

fn check_channel(t: FrameType, channel: u32) -> Result<(), FrameError> {
    match (t, channel) {
        (FrameType::Ctrl, c) if c != 0 => Err(FrameError::CtrlChannel),
        (FrameType::Ctrl, _) => Ok(()),
        (_, 0) => Err(FrameError::ZeroChannel(t.name())),
        _ => Ok(()),
    }
}

impl Frame {
    pub fn new(frame_type: FrameType, channel: u32, payload: impl Into<Bytes>) -> Self {
        Self {
            frame_type,
            flags: 0,
            channel,
            payload: payload.into(),
        }
    }

    /// `CTRL` frame carrying `msg` as compact JSON.
    pub fn ctrl(msg: &Ctrl) -> Result<Self, FrameError> {
        let v = serde_json::to_vec(msg).map_err(|e| FrameError::Json(e.to_string()))?;
        Ok(Self::new(FrameType::Ctrl, 0, v))
    }

    pub fn data_bin(channel: u32, payload: impl Into<Bytes>) -> Self {
        Self::new(FrameType::DataBin, channel, payload)
    }

    /// `DATA_JSON` frame carrying `value` as compact JSON.
    pub fn data_json<T: serde::Serialize>(channel: u32, value: &T) -> Result<Self, FrameError> {
        let v = serde_json::to_vec(value).map_err(|e| FrameError::Json(e.to_string()))?;
        Ok(Self::new(FrameType::DataJson, channel, v))
    }

    /// `WINDOW` frame granting `credit` bytes.
    pub fn window(channel: u32, credit: u32) -> Self {
        Self::new(
            FrameType::Window,
            channel,
            Bytes::copy_from_slice(&credit.to_be_bytes()),
        )
    }

    /// `CLOSE` frame. An empty reason with code 0 encodes as an empty payload.
    pub fn close(channel: u32, code: u16, reason: &str) -> Self {
        let payload = if code == 0 && reason.is_empty() {
            Bytes::new()
        } else {
            let mut b = BytesMut::with_capacity(2 + reason.len());
            b.put_u16(code);
            b.put_slice(reason.as_bytes());
            b.freeze()
        };
        Self::new(FrameType::Close, channel, payload)
    }

    /// Parse the payload of a `CTRL` frame.
    pub fn ctrl_message(&self) -> Result<Ctrl, FrameError> {
        if self.frame_type != FrameType::Ctrl {
            return Err(FrameError::CtrlChannel);
        }
        serde_json::from_slice(&self.payload).map_err(|e| FrameError::Json(e.to_string()))
    }

    /// Parse the payload of a `DATA_JSON` frame.
    pub fn json_value<T: serde::de::DeserializeOwned>(&self) -> Result<T, FrameError> {
        serde_json::from_slice(&self.payload).map_err(|e| FrameError::Json(e.to_string()))
    }

    /// Credit carried by a `WINDOW` frame.
    pub fn window_credit(&self) -> Result<u32, FrameError> {
        let b: [u8; 4] = self.payload[..]
            .try_into()
            .map_err(|_| FrameError::BadWindowPayload)?;
        let c = u32::from_be_bytes(b);
        if c == 0 || c > MAX_CREDIT {
            return Err(FrameError::BadCredit);
        }
        Ok(c)
    }

    /// Code and reason carried by a `CLOSE` frame.
    pub fn close_info(&self) -> Result<CloseInfo, FrameError> {
        match self.payload.len() {
            0 => Ok(CloseInfo {
                code: close_code::NORMAL,
                reason: String::new(),
            }),
            1 => Err(FrameError::BadClosePayload),
            _ => Ok(CloseInfo {
                code: u16::from_be_bytes([self.payload[0], self.payload[1]]),
                reason: std::str::from_utf8(&self.payload[2..])
                    .map_err(|_| FrameError::BadClosePayload)?
                    .to_owned(),
            }),
        }
    }

    /// Check the structural rules for this frame (shared by encode and decode).
    pub fn validate(&self) -> Result<(), FrameError> {
        if self.flags != 0 {
            return Err(FrameError::UnknownFlags(self.flags));
        }
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(FrameError::PayloadTooLarge(self.payload.len()));
        }
        check_channel(self.frame_type, self.channel)?;
        match self.frame_type {
            FrameType::DataBin | FrameType::DataJson if self.payload.is_empty() => {
                Err(FrameError::EmptyData)
            }
            FrameType::Window => self.window_credit().map(|_| ()),
            FrameType::Close => self.close_info().map(|_| ()),
            _ => Ok(()),
        }
    }

    /// Total encoded size.
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }

    /// Append the encoded frame to `dst`.
    pub fn encode(&self, dst: &mut BytesMut) -> Result<(), FrameError> {
        self.validate()?;
        dst.reserve(self.encoded_len());
        dst.put_u8(self.frame_type as u8);
        dst.put_u8(self.flags);
        dst.put_u32(self.channel);
        dst.put_u32(self.payload.len() as u32);
        dst.put_slice(&self.payload);
        Ok(())
    }

    /// Encode into a fresh `Bytes` (one websocket binary message).
    pub fn to_bytes(&self) -> Result<Bytes, FrameError> {
        let mut b = BytesMut::new();
        self.encode(&mut b)?;
        Ok(b.freeze())
    }

    /// Decode exactly one frame from a complete message. The payload is a
    /// zero-copy slice of `msg`.
    pub fn from_bytes(msg: Bytes) -> Result<Self, FrameError> {
        if msg.len() < HEADER_LEN {
            return Err(FrameError::Truncated);
        }
        let h = parse_header(&msg)?;
        let total = HEADER_LEN + h.len;
        if msg.len() < total {
            return Err(FrameError::Truncated);
        }
        if msg.len() > total {
            return Err(FrameError::TrailingBytes(msg.len() - total));
        }
        let f = Self {
            frame_type: h.frame_type,
            flags: h.flags,
            channel: h.channel,
            payload: msg.slice(HEADER_LEN..),
        };
        f.validate()?;
        Ok(f)
    }

    /// Streaming decode: remove one frame from the front of `buf`.
    ///
    /// Returns `Ok(None)` when more bytes are needed. Header errors are
    /// reported as soon as the header is available. After an error the stream
    /// is unrecoverable and the connection must be closed.
    pub fn decode(buf: &mut BytesMut) -> Result<Option<Self>, FrameError> {
        if buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let h = parse_header(buf)?;
        if buf.len() < HEADER_LEN + h.len {
            buf.reserve(HEADER_LEN + h.len - buf.len());
            return Ok(None);
        }
        buf.advance(HEADER_LEN);
        let payload = buf.split_to(h.len).freeze();
        let f = Self {
            frame_type: h.frame_type,
            flags: h.flags,
            channel: h.channel,
            payload,
        };
        f.validate()?;
        Ok(Some(f))
    }
}
