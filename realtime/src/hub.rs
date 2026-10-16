//! Fan-out of change frames to connected clients.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Server to client frame (JSON text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "topic", rename_all = "snake_case")]
pub enum Frame {
    Task {
        id: i64,
        project: String,
    },
    /// Changes may have been missed: refetch everything.
    Resync,
}

impl Frame {
    /// Whether a client allowed to see `projects` should receive this frame.
    pub fn visible_to(&self, projects: &[String]) -> bool {
        match self {
            Frame::Task { project, .. } => projects.iter().any(|p| p == project),
            Frame::Resync => true,
        }
    }
}

/// A bounded broadcast channel; a receiver that falls behind gets `Lagged`, which the
/// websocket loop turns into a `resync` frame.
pub struct Hub {
    tx: broadcast::Sender<Frame>,
    ready: AtomicBool,
}

impl Hub {
    pub fn new(buffer: usize) -> Self {
        Self {
            tx: broadcast::channel(buffer).0,
            ready: AtomicBool::new(false),
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.tx.subscribe()
    }
    pub fn publish(&self, f: Frame) {
        let _ = self.tx.send(f); // no receivers is fine
    }
    pub fn set_ready(&self, v: bool) {
        self.ready.store(v, Ordering::Relaxed);
    }
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_format() {
        let f = Frame::Task {
            id: 123,
            project: "enzarb".into(),
        };
        assert_eq!(
            serde_json::to_string(&f).unwrap(),
            r#"{"topic":"task","id":123,"project":"enzarb"}"#
        );
        assert_eq!(
            serde_json::to_string(&Frame::Resync).unwrap(),
            r#"{"topic":"resync"}"#
        );
    }

    #[test]
    fn project_filtering() {
        let f = Frame::Task {
            id: 1,
            project: "a".into(),
        };
        assert!(f.visible_to(&["a".into(), "b".into()]));
        assert!(!f.visible_to(&["b".into()]));
        assert!(!f.visible_to(&[]));
        assert!(Frame::Resync.visible_to(&[]));
    }
}
