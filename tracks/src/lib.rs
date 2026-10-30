//! enzarb `tracks`: runs ez workflows for enzarb projects (ADR-0029).
//!
//! It serves the API behind the modes and workflow designers (mode documents, workflow
//! validation and rendering, dry runs against logged events) and, with routing on, owns the
//! Postgres schema `tracks`: it routes every connector event to the owning Project's workflows
//! and records a decision per event (`router`). Runs are created `pending`; executing them is a
//! later rollout step.

pub mod api;
pub mod config;
pub mod db;
pub mod exec;
pub mod github;
pub mod projects;
pub mod router;
pub mod runs_api;
pub mod sandbox;
