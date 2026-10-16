//! Browser websocket -> sandbox-init router (contracts §7).
//!
//! Request flow: [`app::mux`] validates the session JWT ([`auth`]), authorizes
//! `sub == spec.owner` through a [`resolver::Resolver`], dials and handshakes the
//! sandbox as `client: "proxy"`, and only then upgrades the browser socket.
//! [`relay`] then forwards frames both ways; [`guard`] enforces that browsers
//! can never send run-control CTRL messages.

pub mod app;
pub mod auth;
pub mod chat;
pub mod config;
pub mod credential;
pub mod guard;
pub mod kube_resolver;
pub mod rekey;
pub mod relay;
pub mod resolver;
pub mod store;
