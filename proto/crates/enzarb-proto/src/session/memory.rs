//! In-memory websocket pair, for tests of code built on [`Session`].

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::{Sink, Stream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::{Events, ServerOptions, Session};
use crate::ClientKind;

/// The other end of a [`MemoryWs`] is gone.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("in-memory websocket closed")]
pub struct MemoryClosed;

/// One end of an in-memory duplex websocket. Closing the sink ends the peer's
/// stream (a clean EOF).
pub struct MemoryWs {
    tx: Option<mpsc::UnboundedSender<Message>>,
    rx: mpsc::UnboundedReceiver<Message>,
}

/// Create two connected [`MemoryWs`] ends.
pub fn pair() -> (MemoryWs, MemoryWs) {
    let (atx, arx) = mpsc::unbounded_channel();
    let (btx, brx) = mpsc::unbounded_channel();
    (
        MemoryWs {
            tx: Some(atx),
            rx: brx,
        },
        MemoryWs {
            tx: Some(btx),
            rx: arx,
        },
    )
}

impl Stream for MemoryWs {
    type Item = Result<Message, MemoryClosed>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|o| o.map(Ok))
    }
}

impl Sink<Message> for MemoryWs {
    type Error = MemoryClosed;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &self.tx {
            Some(tx) if !tx.is_closed() => Poll::Ready(Ok(())),
            _ => Poll::Ready(Err(MemoryClosed)),
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        match &self.tx {
            Some(tx) => tx.send(item).map_err(|_| MemoryClosed),
            None => Err(MemoryClosed),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.tx = None;
        Poll::Ready(Ok(()))
    }
}

/// A connected, handshaken `(client, server)` pair of sessions.
pub async fn session_pair(kind: ClientKind) -> ((Session, Events), (Session, Events)) {
    let (c, s) = pair();
    let (client, server) = tokio::join!(
        Session::connect(c, kind),
        Session::accept(s, ServerOptions::default())
    );
    (
        client.expect("client handshake"),
        server.expect("server handshake"),
    )
}
