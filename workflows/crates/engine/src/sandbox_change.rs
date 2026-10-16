//! Pending sandbox changes (operator rolls), see `ticker::reconcile_holds`.

use enzarb_crds::{ANNOTATION_DRAINED_GENERATION, COND_SANDBOX_READY, Project};

/// What the reconciler needs to know about one Project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectChangeState {
    pub name: String,
    /// `status.pendingChange.generation`.
    pub pending_generation: Option<i64>,
    /// Condition `SandboxReady` is `True`.
    pub sandbox_ready: bool,
    /// Current `enzarb.dev/drained-generation` annotation (raw).
    pub drained: Option<String>,
}

/// Pure projection of a Project, for tests.
pub fn change_state(p: &Project) -> ProjectChangeState {
    let status = p.status.as_ref();
    ProjectChangeState {
        name: p.metadata.name.clone().unwrap_or_default(),
        pending_generation: status
            .and_then(|s| s.pending_change.as_ref())
            .map(|c| c.generation),
        sandbox_ready: status.is_some_and(|s| {
            s.conditions
                .iter()
                .any(|c| c.type_ == COND_SANDBOX_READY && c.status == "True")
        }),
        drained: p
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(ANNOTATION_DRAINED_GENERATION))
            .cloned(),
    }
}
