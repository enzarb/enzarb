//! Contract tests: one single-purpose test per documented promise of a kernel action, run
//! against `FakeConnector`. The registry (`tests/contract-registry.yaml`) maps each promise to
//! its test; see `docs/workflows/contract-tests.md`.

mod create_issues;
mod registry;
