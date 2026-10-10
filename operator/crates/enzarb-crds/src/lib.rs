//! Shared enzarb Kubernetes types: the `Project` CRD (`enzarb.dev/v1alpha1`) and
//! committed kopium-generated types for the agent-sandbox CRD we create.

pub mod project;
pub mod repo;

pub mod upstream {
    //! kopium-generated types, regenerated with `scripts/regen-upstream.sh`.
    pub mod sandbox;
}

pub use project::*;
pub use repo::{RepoKey, RepoKeyError};
