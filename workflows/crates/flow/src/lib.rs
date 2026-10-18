//! `enzarb-flow`: the workflow formats, resolution, templates and the pure
//! interpreter of ADR-0022, shared by the engine, `enzarb-lint`, scenario
//! tests, replay and the UI.
//!
//! The crate contains no workflow-specific knowledge: every phase, step,
//! state name, prompt, event alias and board column is data.
//!
//! - [`format`]: definition file types (workflow, phase, step, runtime, board, bindings).
//! - [`events`]: the standard event catalog.
//! - [`document`]: the task document templates and conditions read.
//! - [`resolve`]: loading through a [`resolve::Loader`], params, exits, layering.
//! - [`template`]: MiniJinja templates and expressions, variable checks.
//! - [`interp`]: `step(resolved, state, event) -> (state, effects)`.
//! - [`lint`] / [`scenario`]: what the `enzarb-lint` binary runs.
//! - [`schema`]: generated JSON Schemas and reference docs.

pub mod commands;
pub mod diag;
pub mod document;
pub mod events;
pub mod format;
pub mod interp;
pub mod lint;
pub mod resolve;
pub mod scenario;
pub mod schema;
pub mod template;
pub mod yaml;

pub use diag::{Diagnostic, Diagnostics};
pub use interp::{Effect, TaskEvent, TaskState, step};
pub use resolve::{Entry, Loader, Resolved, resolve};

#[cfg(test)]
mod tests;
