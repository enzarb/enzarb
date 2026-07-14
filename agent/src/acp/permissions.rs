//! Tracks pending `session/request_permission` calls. Requests are keyed by
//! session id, not by WebSocket connection, so they deliberately survive a
//! client disconnecting: the next client to attach to that session sees the
//! pending prompt and can answer it (no timeout/auto-deny in v1).

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

#[derive(Default, Clone)]
pub struct PermissionRegistry {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
}

impl PermissionRegistry {
    /// Registers a pending request and returns its id plus a receiver that
    /// resolves with the chosen `option_id` once a client responds.
    pub async fn register(&self) -> (String, oneshot::Receiver<String>) {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        (id, rx)
    }

    /// Resolves a pending request with the option id the client chose.
    /// Returns false if no such request was pending (already answered, or unknown).
    pub async fn resolve(&self, request_id: &str, option_id: String) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(request_id) {
            tx.send(option_id).is_ok()
        } else {
            false
        }
    }
}

/// Tool-kind based auto-allow policy: reads are auto-allowed, everything else
/// (edit/execute/other) requires an explicit prompt.
pub fn auto_allow(tool_kind: &str) -> bool {
    tool_kind == "read"
}

/// A client's elicitation answers: `field_key` -> selected value(s).
/// `None` means the elicitation was declined/skipped entirely.
type ElicitationAnswers = Option<HashMap<String, serde_json::Value>>;

/// Tracks pending `elicitation/create` calls (currently only used for the
/// AskUserQuestion tool's form elicitation). Same session-keyed, survives-
/// disconnect design as [`PermissionRegistry`] — see its doc comment.
#[derive(Default, Clone)]
pub struct ElicitationRegistry {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ElicitationAnswers>>>>,
}

impl ElicitationRegistry {
    /// Registers a pending elicitation and returns its id plus a receiver that
    /// resolves with the client's answers (`None` means declined/skipped).
    pub async fn register(&self) -> (String, oneshot::Receiver<ElicitationAnswers>) {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        (id, rx)
    }

    /// Resolves a pending elicitation with the client's answers.
    /// Returns false if no such request was pending (already answered, or unknown).
    pub async fn resolve(&self, request_id: &str, answers: ElicitationAnswers) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(request_id) {
            tx.send(answers).is_ok()
        } else {
            false
        }
    }
}
