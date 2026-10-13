//! The LISTEN side: a supervisor that keeps one notification stream alive and publishes frames.
//! Postgres is behind traits so tests can drive it with channels.

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use async_trait::async_trait;

use crate::hub::{Frame, Hub};

/// A live `LISTEN work_item_changed` stream.
#[async_trait]
pub trait Listener: Send {
    /// Next work item id. `Err` means the connection is lost and must be re-established.
    async fn recv(&mut self) -> Result<i64>;
}

#[async_trait]
pub trait Connector: Send + Sync {
    /// Connect and `LISTEN`.
    async fn connect(&self) -> Result<Box<dyn Listener>>;
}

/// Work item id to project name. `None` when the item is not visible/does not exist.
#[async_trait]
pub trait ProjectLookup: Send + Sync {
    async fn project(&self, work_item_id: i64) -> Result<Option<String>>;
}

#[derive(Clone, Copy)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(30),
        }
    }
}

/// Run forever. After any connection loss or failed attempt, a `resync` is broadcast once the
/// listener is re-established, because notifications sent in between are gone.
pub async fn run(
    connector: Arc<dyn Connector>,
    lookup: Arc<dyn ProjectLookup>,
    hub: Arc<Hub>,
    backoff: Backoff,
) {
    let mut delay = backoff.initial;
    let mut needs_resync = false;
    loop {
        let mut l = match connector.connect().await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, retry_ms = delay.as_millis() as u64, "listen connect failed");
                needs_resync = true;
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(backoff.max);
                continue;
            }
        };
        let connected_at = tokio::time::Instant::now();
        hub.set_ready(true);
        tracing::info!("listening on work_item_changed");
        if needs_resync {
            hub.publish(Frame::Resync);
        }
        loop {
            match l.recv().await {
                Ok(id) => hub.publish(frame_for(id, lookup.as_ref()).await),
                Err(e) => {
                    tracing::warn!(error = %e, "listen connection lost");
                    hub.set_ready(false);
                    needs_resync = true;
                    // Only forget the backoff after a healthy stretch, so a flapping connection
                    // cannot spin and spam clients with resyncs.
                    if connected_at.elapsed() > Duration::from_secs(10) {
                        delay = backoff.initial;
                    }
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(backoff.max);
                    break;
                }
            }
        }
    }
}

/// A failed or empty lookup degrades to `resync` (never to an unfiltered work_item frame).
async fn frame_for(id: i64, lookup: &dyn ProjectLookup) -> Frame {
    let r = tokio::time::timeout(Duration::from_secs(2), lookup.project(id))
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("lookup timed out")));
    match r {
        Ok(Some(project)) => Frame::WorkItem { id, project },
        Ok(None) => {
            tracing::debug!(id, "work item has no project; resync");
            Frame::Resync
        }
        Err(e) => {
            tracing::warn!(id, error = %e, "project lookup failed; resync");
            Frame::Resync
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};
    use tokio::sync::mpsc;

    struct Fake(mpsc::UnboundedReceiver<Result<i64>>);
    #[async_trait]
    impl Listener for Fake {
        async fn recv(&mut self) -> Result<i64> {
            self.0
                .recv()
                .await
                .unwrap_or_else(|| Err(anyhow::anyhow!("closed")))
        }
    }
    struct Script(Mutex<VecDeque<Result<Fake>>>);
    #[async_trait]
    impl Connector for Script {
        async fn connect(&self) -> Result<Box<dyn Listener>> {
            let next = self.0.lock().unwrap().pop_front();
            match next {
                Some(r) => r.map(|f| Box::new(f) as Box<dyn Listener>),
                None => std::future::pending().await,
            }
        }
    }
    struct Map;
    #[async_trait]
    impl ProjectLookup for Map {
        async fn project(&self, id: i64) -> Result<Option<String>> {
            match id {
                1 => Ok(Some("enzarb".into())),
                2 => Ok(None),
                _ => Err(anyhow::anyhow!("db down")),
            }
        }
    }

    const FAST: Backoff = Backoff {
        initial: Duration::from_millis(1),
        max: Duration::from_millis(4),
    };

    #[tokio::test]
    async fn notify_becomes_frame_and_reconnect_resyncs() {
        let (tx1, rx1) = mpsc::unbounded_channel();
        let (tx2, rx2) = mpsc::unbounded_channel();
        let script = Script(Mutex::new(VecDeque::from([
            Ok(Fake(rx1)),
            Err(anyhow::anyhow!("refused")),
            Ok(Fake(rx2)),
        ])));
        let hub = Arc::new(Hub::new(16));
        let mut sub = hub.subscribe();
        tokio::spawn(run(Arc::new(script), Arc::new(Map), hub.clone(), FAST));

        tx1.send(Ok(1)).unwrap();
        assert_eq!(
            sub.recv().await.unwrap(),
            Frame::WorkItem {
                id: 1,
                project: "enzarb".into()
            }
        );
        // Unknown project and lookup errors degrade to resync, not to an unfiltered frame.
        tx1.send(Ok(2)).unwrap();
        assert_eq!(sub.recv().await.unwrap(), Frame::Resync);
        tx1.send(Ok(3)).unwrap();
        assert_eq!(sub.recv().await.unwrap(), Frame::Resync);

        // Connection lost, one failed reconnect, then success: exactly one resync.
        tx1.send(Err(anyhow::anyhow!("reset"))).unwrap();
        assert_eq!(sub.recv().await.unwrap(), Frame::Resync);
        assert!(hub.is_ready());
        tx2.send(Ok(1)).unwrap();
        assert!(matches!(
            sub.recv().await.unwrap(),
            Frame::WorkItem { id: 1, .. }
        ));
    }

    #[tokio::test]
    async fn no_resync_on_clean_first_connect() {
        let (tx, rx) = mpsc::unbounded_channel();
        let script = Script(Mutex::new(VecDeque::from([Ok(Fake(rx))])));
        let hub = Arc::new(Hub::new(16));
        let mut sub = hub.subscribe();
        tokio::spawn(run(Arc::new(script), Arc::new(Map), hub, FAST));
        tx.send(Ok(1)).unwrap();
        assert!(matches!(
            sub.recv().await.unwrap(),
            Frame::WorkItem { id: 1, .. }
        ));
    }
}
