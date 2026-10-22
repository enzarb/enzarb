//! enzarb `tracks`: runs ez workflows for enzarb projects (ADR-0029).
//!
//! Today it serves the stateless API behind the modes and workflow designers: mode documents
//! (ez-core's editor functions), workflow validation and rendering, and dry runs against logged
//! events (`ez-workflow`). Event routing and real runs come later.

pub mod api;
pub mod config;
