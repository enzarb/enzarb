use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use super::{ConnectError, Events, Session};
use crate::ClientKind;

/// Connection helper for clients of `sandbox-init` (`ws://<pod>:7070/v1/mux`).
pub struct Client;

impl Client {
    /// Dial `url`, authenticate the upgrade with `Authorization: Bearer
    /// <bearer_token>`, then run the client handshake as `client_kind`.
    ///
    /// `wss://` URLs need the `tls` cargo feature.
    pub async fn connect(
        url: &str,
        bearer_token: &str,
        client_kind: ClientKind,
    ) -> Result<(Session, Events), ConnectError> {
        let mut req = url.into_client_request()?;
        let value = HeaderValue::from_str(&format!("Bearer {bearer_token}"))
            .map_err(|_| ConnectError::BadToken)?;
        req.headers_mut().insert("authorization", value);
        let (ws, _resp) = connect_async(req).await?;
        Ok(Session::connect(ws, client_kind).await?)
    }
}
