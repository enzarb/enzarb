//! Thin adapter over the official ACP SDK (`agent-client-protocol`) JSON-RPC connection layer.
//!
//! The SDK owns framing, request ids, response routing, error replies and the dispatch loop.
//! This module only keeps two things:
//!
//! - a method-string + `serde_json::Value` view ([`Handler`], [`Connection`]) so that extension
//!   methods (`_ez/*`), `_meta` and our lenient parsing stay independent of the typed
//!   schema, and
//! - the transport: newline-delimited JSON over tokio streams, where blank and non-JSON lines
//!   are skipped (the SDK would answer them with a `-32700` error).
//!
//! Ordering: the SDK runs notification callbacks inline in its dispatch loop and routes
//! responses in the same loop, so a response never resolves before earlier notifications were
//! handled. A notification handler must therefore not await a request on the same connection.
//! Requests are handled on their own tokio tasks, so a long-running `session/prompt` does not
//! block `session/cancel` or the requests it makes back to the peer.

use std::sync::Arc;

use agent_client_protocol as sdk;
use agent_client_protocol::role::HasPeer;
use agent_client_protocol::{ConnectTo, ConnectionTo, Lines, Responder, Role, UntypedMessage};
use async_trait::async_trait;
use futures::StreamExt;
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{oneshot, watch};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message} ({code})")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
    pub fn method_not_found(method: &str) -> Self {
        Self::new(-32601, format!("method not found: {method}"))
    }
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(-32602, msg)
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(-32603, msg)
    }
}

impl From<anyhow::Error> for RpcError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(format!("{e:#}"))
    }
}

impl From<RpcError> for sdk::Error {
    fn from(e: RpcError) -> Self {
        let code = i32::try_from(e.code).unwrap_or(-32603);
        let mut err = sdk::Error::new(code, e.message);
        err.data = e.data;
        err
    }
}

impl From<sdk::Error> for RpcError {
    fn from(e: sdk::Error) -> Self {
        RpcError {
            code: i64::from(i32::from(e.code)),
            message: e.message,
            data: e.data,
        }
    }
}

#[async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError>;
    /// Handle a notification. Notifications for one connection are handled strictly in arrival
    /// order, and responses to our own requests are routed in the same order (after all earlier
    /// notifications).
    ///
    /// Therefore a notification handler MUST NOT await a request on the same connection: the
    /// response could never be delivered and it would deadlock. Spawn a task instead if a
    /// request is needed.
    async fn notification(&self, method: &str, params: Value);
}

/// Role-erased sending half of an SDK connection.
trait Peer: Send + Sync {
    fn request(&self, msg: UntypedMessage) -> BoxFuture<'static, Result<Value, sdk::Error>>;
    fn notify(&self, msg: UntypedMessage) -> Result<(), sdk::Error>;
}

impl<R: Role + HasPeer<R>> Peer for ConnectionTo<R> {
    fn request(&self, msg: UntypedMessage) -> BoxFuture<'static, Result<Value, sdk::Error>> {
        Box::pin(self.send_request(msg).block_task())
    }
    fn notify(&self, msg: UntypedMessage) -> Result<(), sdk::Error> {
        self.send_notification(msg)
    }
}

type PeerSlot = Option<Arc<dyn Peer>>;

/// Cheaply clonable handle to one end of a JSON-RPC connection. It can be created before the
/// connection is running (see [`Link`]); requests wait until it is up.
#[derive(Clone)]
pub struct Connection {
    peer: watch::Receiver<PeerSlot>,
    /// Client-side handles keep the connection alive: it is shut down when the last one drops.
    _keepalive: Option<Arc<oneshot::Sender<()>>>,
}

/// Binds a [`Connection`] to the running SDK connection: pass to [`run`].
pub struct Link(watch::Sender<PeerSlot>);

impl Connection {
    /// A not-yet-running connection and the [`Link`] that brings it up.
    pub fn link() -> (Link, Connection) {
        let (tx, rx) = watch::channel(None);
        (
            Link(tx),
            Connection {
                peer: rx,
                _keepalive: None,
            },
        )
    }

    async fn peer(&self) -> Result<Arc<dyn Peer>, RpcError> {
        let mut rx = self.peer.clone();
        let slot = rx
            .wait_for(Option::is_some)
            .await
            .map_err(|_| RpcError::internal("connection closed"))?;
        Ok(slot.clone().expect("checked by wait_for"))
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let msg = untyped(method, params)?;
        Ok(self.peer().await?.request(msg).await?)
    }

    pub async fn request_typed<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, RpcError> {
        let params =
            serde_json::to_value(params).map_err(|e| RpcError::invalid_params(e.to_string()))?;
        let v = self.request(method, params).await?;
        serde_json::from_value(v).map_err(|e| RpcError::internal(format!("bad response: {e}")))
    }

    /// Fire-and-forget; messages go out in call order.
    pub fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        let msg = untyped(method, params)?;
        let peer = self.peer.borrow().clone();
        match peer {
            Some(p) => Ok(p.notify(msg)?),
            None => Err(RpcError::internal("connection not ready")),
        }
    }
}

fn untyped(method: &str, params: Value) -> Result<UntypedMessage, RpcError> {
    UntypedMessage::new(method, params).map_err(|e| RpcError::invalid_params(e.to_string()))
}

/// The SDK's version guard insists on a valid `protocolVersion` in `initialize` and rewrites
/// requests for a newer version (dropping capabilities it does not know). We negotiate
/// `min(requested, 1)` ourselves and treat a missing version as v1, so clamp it up front.
fn lenient_initialize(mut v: Value, line: String) -> String {
    let is_init = v.get("method").and_then(Value::as_str) == Some("initialize")
        && v.get("id").is_some_and(|id| !id.is_null());
    if !is_init {
        return line;
    }
    let Some(params) = v.get_mut("params").and_then(Value::as_object_mut) else {
        return line;
    };
    match params.get("protocolVersion").and_then(Value::as_u64) {
        Some(0 | 1) => return line,
        _ => {
            params.insert("protocolVersion".into(), Value::from(1));
        }
    }
    v.to_string()
}

/// Newline-delimited JSON over tokio streams. Blank and non-JSON lines are dropped.
pub fn lines_transport<R, W>(
    reader: R,
    writer: W,
) -> Lines<
    impl futures::Sink<String, Error = std::io::Error> + Send + 'static,
    impl futures::Stream<Item = std::io::Result<String>> + Send + 'static,
>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    use futures::AsyncBufReadExt;
    use futures::AsyncWriteExt;
    let incoming = futures::io::BufReader::new(reader.compat())
        .lines()
        .filter_map(|l| {
            futures::future::ready(match l {
                Ok(l) if l.trim().is_empty() => None,
                Ok(l) => match serde_json::from_str::<Value>(&l) {
                    Ok(v) => Some(Ok(lenient_initialize(v, l))),
                    Err(_) => {
                        tracing::warn!("malformed rpc line ignored");
                        None
                    }
                },
                Err(e) => Some(Err(e)),
            })
        });
    let outgoing = futures::sink::unfold(writer.compat_write(), |mut w, line: String| async move {
        let mut buf = line.into_bytes();
        buf.push(b'\n');
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok::<_, std::io::Error>(w)
    });
    Lines::new(Box::pin(outgoing), Box::pin(incoming))
}

/// Run one end of a connection until the peer disconnects (or `stop` fires/closes). `link`
/// brings the paired [`Connection`] up.
pub async fn run<Host>(
    host: Host,
    transport: impl ConnectTo<Host> + 'static,
    handler: Arc<dyn Handler>,
    link: Link,
    stop: Option<oneshot::Receiver<()>>,
) -> Result<(), sdk::Error>
where
    Host: Role,
    Host::Counterpart: HasPeer<Host::Counterpart>,
{
    let on_req = handler.clone();
    let on_notif = handler;
    host.builder()
        .on_receive_request(
            async move |msg: UntypedMessage,
                        responder: Responder<Value>,
                        _cx: ConnectionTo<Host::Counterpart>| {
                let h = on_req.clone();
                tokio::spawn(async move {
                    let (method, params) = msg.into_parts();
                    let _ = match h.request(&method, params).await {
                        Ok(v) => responder.respond(v),
                        Err(e) => responder.respond_with_error(e.into()),
                    };
                });
                Ok(())
            },
            sdk::on_receive_request!(),
        )
        .on_receive_notification(
            async move |msg: UntypedMessage, _cx: ConnectionTo<Host::Counterpart>| {
                let (method, params) = msg.into_parts();
                on_notif.notification(&method, params).await;
                Ok(())
            },
            sdk::on_receive_notification!(),
        )
        .connect_with(
            transport,
            async move |cx: ConnectionTo<Host::Counterpart>| {
                link.0.send_replace(Some(Arc::new(cx.clone())));
                match stop {
                    Some(stop) => {
                        futures::future::select(Box::pin(cx.incoming_closed()), Box::pin(stop))
                            .await;
                    }
                    None => cx.incoming_closed().await,
                }
                Ok(())
            },
        )
        .await
}

/// Start the client end of a connection in the background and return a handle to it. The
/// connection lives until the last clone of the handle is dropped or the peer disconnects.
pub fn spawn_client(
    transport: impl ConnectTo<sdk::Client> + 'static,
    handler: Arc<dyn Handler>,
) -> Connection {
    let (link, mut conn) = Connection::link();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    conn._keepalive = Some(Arc::new(stop_tx));
    tokio::spawn(async move {
        if let Err(e) = run(sdk::Client, transport, handler, link, Some(stop_rx)).await {
            tracing::debug!("acp client connection ended: {e}");
        }
    });
    conn
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Mutex, OnceLock};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    struct Echo {
        peer: OnceLock<Connection>,
        seen: Mutex<Vec<i64>>,
    }
    impl Echo {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                peer: OnceLock::new(),
                seen: Mutex::new(vec![]),
            })
        }
    }
    #[async_trait]
    impl Handler for Echo {
        async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
            match method {
                "echo" | "initialize" => Ok(params),
                "callback" => {
                    // Call back into the peer while handling.
                    let r = self
                        .peer
                        .get()
                        .unwrap()
                        .request("echo", json!({"v": "hi"}))
                        .await?;
                    Ok(json!({ "peer": r["v"] }))
                }
                m => Err(RpcError::method_not_found(m)),
            }
        }
        async fn notification(&self, _m: &str, params: Value) {
            self.seen
                .lock()
                .unwrap()
                .push(params["i"].as_i64().unwrap());
        }
    }

    /// Agent-role end running `a`, client-role end running `b`, over an in-memory pipe.
    fn pair(a: Arc<dyn Handler>, b: Arc<dyn Handler>) -> (Connection, Connection) {
        let (x, y) = tokio::io::duplex(64 * 1024);
        let (xr, xw) = tokio::io::split(x);
        let (yr, yw) = tokio::io::split(y);
        let (link, ca) = Connection::link();
        tokio::spawn(async move {
            let _ = run(sdk::Agent, lines_transport(xr, xw), a, link, None).await;
        });
        (ca, spawn_client(lines_transport(yr, yw), b))
    }

    #[tokio::test]
    async fn round_trip_and_callback_no_deadlock() {
        let (ha, hb) = (Echo::new(), Echo::new());
        let (a, b) = pair(ha.clone(), hb.clone());
        ha.peer.set(b.clone()).ok();
        hb.peer.set(a.clone()).ok();
        let (r1, r2) = tokio::join!(
            a.request("callback", Value::Null),
            b.request("callback", Value::Null)
        );
        assert_eq!(r1.unwrap(), json!({"peer":"hi"}));
        assert_eq!(r2.unwrap(), json!({"peer":"hi"}));
        let t: Value = a.request_typed("echo", &json!({"v": "x"})).await.unwrap();
        assert_eq!(t["v"], "x");
    }

    #[tokio::test]
    async fn unknown_method_and_notification_order() {
        let (ha, hb) = (Echo::new(), Echo::new());
        let (a, _b) = pair(ha, hb.clone());
        let e = a.request("nope", Value::Null).await.unwrap_err();
        assert_eq!(e.code, -32601);
        for i in 0..50 {
            a.notify("n", json!({ "i": i })).unwrap();
        }
        a.request("echo", Value::Null).await.unwrap();
        let seen = hb.seen.lock().unwrap().clone();
        assert_eq!(seen, (0..50).collect::<Vec<_>>());
    }

    struct Slow {
        seen: Mutex<Vec<i64>>,
    }
    #[async_trait]
    impl Handler for Slow {
        async fn request(&self, _m: &str, _p: Value) -> Result<Value, RpcError> {
            Ok(Value::Null)
        }
        async fn notification(&self, _m: &str, params: Value) {
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            self.seen
                .lock()
                .unwrap()
                .push(params["i"].as_i64().unwrap());
        }
    }

    #[tokio::test]
    async fn response_after_all_prior_notifications_handled() {
        // Peer emits N notifications then a response in one burst; our handler is slow.
        let client = Arc::new(Slow {
            seen: Mutex::new(vec![]),
        });
        let (x, y) = tokio::io::duplex(64 * 1024);
        let (xr, xw) = tokio::io::split(x);
        let (mut yr, mut yw) = tokio::io::split(y);
        let c = spawn_client(lines_transport(xr, xw), client.clone());
        let fut = tokio::spawn({
            let c = c.clone();
            async move { c.request("go", Value::Null).await }
        });
        // Read the request id, then reply with notifications + response in one burst.
        let mut line = String::new();
        let mut br = BufReader::new(&mut yr);
        br.read_line(&mut line).await.unwrap();
        let id = serde_json::from_str::<Value>(&line).unwrap()["id"].clone();
        let mut out = String::new();
        for i in 0..40 {
            out += &format!("{{\"jsonrpc\":\"2.0\",\"method\":\"n\",\"params\":{{\"i\":{i}}}}}\n");
        }
        out += &json!({"jsonrpc":"2.0","id":id,"result":1}).to_string();
        out.push('\n');
        yw.write_all(out.as_bytes()).await.unwrap();
        fut.await.unwrap().unwrap();
        assert_eq!(client.seen.lock().unwrap().len(), 40);
    }

    #[tokio::test]
    async fn pending_fails_when_peer_closes() {
        let (x, y) = tokio::io::duplex(1024);
        let (xr, xw) = tokio::io::split(x);
        let a = spawn_client(lines_transport(xr, xw), Echo::new());
        let req = tokio::spawn({
            let a = a.clone();
            async move { a.request("echo", Value::Null).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        drop(y);
        let err = req.await.unwrap().unwrap_err();
        assert!(err.code != 0, "{err}");
        assert!(a.request("echo", Value::Null).await.is_err());
    }

    /// Feed raw lines to an agent-role end and return what it wrote back.
    async fn raw_exchange(input: &str, expect_lines: usize) -> Vec<Value> {
        let (x, mut y) = tokio::io::duplex(4096);
        let (xr, xw) = tokio::io::split(x);
        let (link, _conn) = Connection::link();
        tokio::spawn(async move {
            let _ = run(sdk::Agent, lines_transport(xr, xw), Echo::new(), link, None).await;
        });
        y.write_all(input.as_bytes()).await.unwrap();
        let mut lines = BufReader::new(y).lines();
        let mut out = vec![];
        for _ in 0..expect_lines {
            let l = lines.next_line().await.unwrap().unwrap();
            out.push(serde_json::from_str(&l).unwrap());
        }
        out
    }

    #[tokio::test]
    async fn malformed_and_blank_lines_skipped() {
        let out = raw_exchange(
            "garbage\n\n   \n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"echo\",\"params\":{\"a\":[1]}}\n",
            1,
        )
        .await;
        assert_eq!(out[0]["id"], 7);
        assert_eq!(out[0]["result"], json!({"a":[1]}));
    }

    #[tokio::test]
    async fn lenient_initialize_version() {
        // Handler echoes params, so we see what the SDK passed on.
        let out = raw_exchange(
            concat!(
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
                "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"initialize\",\"params\":{\"protocolVersion\":99,\"clientCapabilities\":{\"fs\":{\"readTextFile\":true}}}}\n",
            ),
            2,
        )
        .await;
        let by_id = |id: i64| out.iter().find(|v| v["id"] == id).unwrap();
        assert!(by_id(1).get("result").is_some(), "{out:?}");
        assert!(by_id(2).get("result").is_some(), "{out:?}");
        assert_eq!(
            by_id(2)["result"]["clientCapabilities"]["fs"]["readTextFile"],
            true
        );
    }

    #[tokio::test]
    async fn extension_meta_preserved() {
        let (ha, hb) = (Echo::new(), Echo::new());
        let (a, _b) = pair(ha, hb);
        let p = json!({"_meta":{"ez":{"x":true}},"v":[1,{"k":null}]});
        assert_eq!(a.request("echo", p.clone()).await.unwrap(), p);
    }
}
