//! enzarb-realtime: Postgres `NOTIFY engine_projection` to an authenticated browser websocket.

pub mod auth;
pub mod config;
pub mod hub;
pub mod listener;
pub mod pg;
pub mod ws;
