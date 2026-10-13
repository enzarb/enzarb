//! hello/welcome handshake (protocol.md section 6), usable on its own.
//!
//! [`Session::connect`](super::Session::connect) and
//! [`Session::accept`](super::Session::accept) call these. A forwarder such as
//! sandbox-proxy can call them directly on a raw websocket and then relay the
//! remaining messages without ever creating a `Session`.

use std::fmt::Display;

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use super::HandshakeError;
use crate::{
    ClientKind, Ctrl, Frame, SUPPORTED_VERSIONS, error_code, frame::FrameType, negotiate_version,
    sanitize_features,
};

/// Result of a successful server-side handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHello {
    /// Negotiated version (already sent in `welcome`).
    pub version: u16,
    pub client: ClientKind,
    /// Versions the client offered.
    pub offered: Vec<u16>,
    /// `hello.features`, sanitized. Empty when absent.
    pub features: Vec<String>,
}

async fn send_ctrl<S, E>(ws: &mut S, m: &Ctrl) -> Result<(), HandshakeError>
where
    S: Sink<Message, Error = E> + Unpin,
    E: Display,
{
    let b = Frame::ctrl(m)
        .and_then(|f| f.to_bytes())
        .map_err(|e| HandshakeError::Protocol(e.to_string()))?;
    ws.send(Message::Binary(b))
        .await
        .map_err(|e| HandshakeError::Transport(e.to_string()))
}

/// Next frame, skipping websocket pings/pongs. `Ok(None)` on close.
async fn next_frame<S, E>(ws: &mut S) -> Result<Option<Frame>, String>
where
    S: Stream<Item = Result<Message, E>> + Unpin,
    E: Display,
{
    loop {
        match ws.next().await {
            None | Some(Ok(Message::Close(_))) => return Ok(None),
            Some(Err(e)) => return Err(format!("websocket: {e}")),
            Some(Ok(Message::Binary(b))) => {
                return Frame::from_bytes(b).map(Some).map_err(|e| e.to_string());
            }
            Some(Ok(Message::Text(_))) => return Err("text message".into()),
            Some(Ok(_)) => {}
        }
    }
}

async fn reject<S, E>(ws: &mut S, code: &str, message: String) -> HandshakeError
where
    S: Sink<Message, Error = E> + Unpin,
    E: Display,
{
    let _ = send_ctrl(
        ws,
        &Ctrl::Error {
            code: code.into(),
            message: message.clone(),
            run_id: None,
            channel: None,
        },
    )
    .await;
    let _ = ws.close().await;
    HandshakeError::Protocol(message)
}

/// Server side: wait for `hello`, pick the highest common version in
/// `supported`, reply `welcome`.
///
/// On any failure an `error` is sent (best effort) and the websocket is closed.
pub async fn server_handshake<S, E>(
    ws: &mut S,
    supported: &[u16],
) -> Result<ServerHello, HandshakeError>
where
    S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin,
    E: Display,
{
    server_handshake_with_features(ws, supported, &[]).await
}

/// Like [`server_handshake`], advertising `features` in `welcome` (omitted
/// from the wire when empty).
pub async fn server_handshake_with_features<S, E>(
    ws: &mut S,
    supported: &[u16],
    features: &[String],
) -> Result<ServerHello, HandshakeError>
where
    S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin,
    E: Display,
{
    let frame = match next_frame(ws).await {
        Ok(Some(f)) => f,
        Ok(None) => return Err(HandshakeError::Closed),
        Err(e) => return Err(reject(ws, error_code::PROTOCOL_VIOLATION, e).await),
    };
    let (versions, client, peer_features) = match (frame.frame_type, frame.ctrl_message()) {
        (
            FrameType::Ctrl,
            Ok(Ctrl::Hello {
                versions,
                client,
                features,
            }),
        ) => (versions, client, sanitize_features(features)),
        _ => {
            let msg = "first frame must be CTRL hello".to_string();
            return Err(reject(ws, error_code::PROTOCOL_VIOLATION, msg).await);
        }
    };
    let Some(version) = negotiate_version(&versions, supported) else {
        let msg = format!("no common version (client offered {versions:?})");
        let _ = reject(ws, error_code::UNSUPPORTED_VERSION, msg).await;
        return Err(HandshakeError::UnsupportedVersion { offered: versions });
    };
    let features = (!features.is_empty()).then(|| features.to_vec());
    send_ctrl(ws, &Ctrl::Welcome { version, features }).await?;
    Ok(ServerHello {
        version,
        client,
        offered: versions,
        features: peer_features,
    })
}

/// Client side: send `hello` offering [`SUPPORTED_VERSIONS`] and wait for
/// `welcome`. Returns the negotiated version.
pub async fn client_handshake<S, E>(ws: &mut S, kind: ClientKind) -> Result<u16, HandshakeError>
where
    S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin,
    E: Display,
{
    Ok(client_handshake_with_features(ws, kind).await?.version)
}

/// What a client learns from `welcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientWelcome {
    pub version: u16,
    /// The server's advertised features, sanitized with
    /// [`sanitize_features`](crate::sanitize_features). Empty when absent.
    pub features: Vec<String>,
}

/// Like [`client_handshake`] but also returns `welcome.features`.
pub async fn client_handshake_with_features<S, E>(
    ws: &mut S,
    kind: ClientKind,
) -> Result<ClientWelcome, HandshakeError>
where
    S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin,
    E: Display,
{
    client_handshake_offering(ws, kind, &[]).await
}

/// Like [`client_handshake_with_features`], also offering `features` in
/// `hello` (omitted from the wire when empty).
pub async fn client_handshake_offering<S, E>(
    ws: &mut S,
    kind: ClientKind,
    features: &[String],
) -> Result<ClientWelcome, HandshakeError>
where
    S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin,
    E: Display,
{
    send_ctrl(
        ws,
        &Ctrl::Hello {
            versions: SUPPORTED_VERSIONS.to_vec(),
            client: kind,
            features: (!features.is_empty()).then(|| features.to_vec()),
        },
    )
    .await?;
    let frame = match next_frame(ws).await {
        Ok(Some(f)) => f,
        Ok(None) => return Err(HandshakeError::Closed),
        Err(e) => return Err(HandshakeError::Protocol(e)),
    };
    match (frame.frame_type, frame.ctrl_message()) {
        (FrameType::Ctrl, Ok(Ctrl::Welcome { version, features }))
            if SUPPORTED_VERSIONS.contains(&version) =>
        {
            Ok(ClientWelcome {
                version,
                features: sanitize_features(features),
            })
        }
        (FrameType::Ctrl, Ok(Ctrl::Welcome { version, .. })) => Err(HandshakeError::Protocol(
            format!("server chose unsupported version {version}"),
        )),
        (FrameType::Ctrl, Ok(Ctrl::Error { code, message, .. })) => {
            let _ = ws.close().await;
            Err(HandshakeError::Rejected { code, message })
        }
        _ => {
            let msg = "expected CTRL welcome".to_string();
            Err(reject(ws, error_code::PROTOCOL_VIOLATION, msg).await)
        }
    }
}
