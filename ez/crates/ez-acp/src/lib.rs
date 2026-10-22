//! Agent Client Protocol (v1) agent over a bidirectional JSON-RPC 2.0 transport.
//!
//! Wire types come from the official `agent-client-protocol-schema` crate (see [`schema`]) and
//! the JSON-RPC connection layer from the official `agent-client-protocol` SDK ([`rpc`] is a thin
//! adapter over it); the method dispatch ([`agent`]) is ours.

pub mod agent;
pub mod rpc;
pub mod schema;

pub use agent::{AcpAgent, serve, serve_stdio, spawn_in_process, spawn_in_process_raw};
pub use rpc::{Connection, Handler, RpcError};
pub use schema::ProtocolVersion;

/// ACP protocol version implemented by this crate.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V1;
