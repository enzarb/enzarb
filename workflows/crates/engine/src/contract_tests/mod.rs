//! Contract tests: one single-purpose test per documented promise of a kernel action, run
//! against `FakeConnector`. The registry (`tests/contract-registry.yaml`) maps each promise to
//! its test; see `docs/workflows/contract-tests.md`.

mod close_issue;
mod comment;
mod common;
mod create_issues;
mod delete_branch;
mod facts;
mod merge_pull_request;
mod open_pull_request;
mod read_file;
mod registry;
mod update_branch;
