#![cfg(feature = "tokio")]
#![allow(clippy::result_large_err)]
//! `Client::connect` against a real tokio-tungstenite server on loopback.

use enzarb_proto::ClientKind;
use enzarb_proto::session::{Client, Event, ServerOptions, Session};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

#[tokio::test]
async fn connect_sends_bearer_and_handshakes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut auth = None;
        let ws = accept_hdr_async(
            tcp,
            |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                auth = req
                    .headers()
                    .get("authorization")
                    .map(|v| v.to_str().unwrap().to_owned());
                Ok(resp)
            },
        )
        .await
        .unwrap();
        let (session, mut events) = Session::accept(ws, ServerOptions::default()).await.unwrap();
        let Some(Event::Open(req)) = events.next().await else {
            panic!()
        };
        let mut ch = req.accept().unwrap();
        let mut buf = [0u8; 5];
        ch.read_exact(&mut buf).await.unwrap();
        ch.write_all(&buf).await.unwrap();
        let kind = session.peer_kind();
        let _ = session.closed().await;
        (auth, kind)
    });

    let (session, _events) = Client::connect(
        &format!("ws://{addr}/v1/mux"),
        "s3cret",
        ClientKind::Executor,
    )
    .await
    .unwrap();
    let mut ch = session.open_pty(80, 24, None).await.unwrap();
    ch.write_all(b"hello").await.unwrap();
    let mut buf = [0u8; 5];
    ch.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello");
    session.close().await;
    let (auth, kind) = server.await.unwrap();
    assert_eq!(auth.as_deref(), Some("Bearer s3cret"));
    assert_eq!(kind, Some(ClientKind::Executor));
}
