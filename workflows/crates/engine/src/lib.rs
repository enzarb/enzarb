//! enzarb workflow engine library: schema access, state machines, reducer, ticker,
//! job-result ingestion and executors.

pub mod conflict;
pub mod db;
pub mod error;
pub mod executor;
pub mod grants;
pub mod index;
pub mod machine;
pub mod permission;
pub mod reducer;
pub mod results;
#[cfg(feature = "testing")]
pub mod testing;
pub mod ticker;

pub use error::Error;
pub use executor::{
    ClaudeKeyring, ConnectorRegistry, Executor, ExecutorConfig, SandboxResolver, SandboxTarget,
    Secret,
};
pub use index::{KubeProjectIndex, ProjectIndex, ProjectInfo, StaticProjectIndex};
pub use permission::{
    CachedPermissions, ConnectorPermissions, PermissionResolver, StaticPermissions,
};
pub use reducer::{Reducer, ReducerConfig};
pub use results::{JobResult, ingest_job_result};
pub use ticker::{TickStats, Ticker};
