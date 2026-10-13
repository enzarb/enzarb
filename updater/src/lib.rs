//! enzarb-updater: turns a published `enzarb-helm` GitHub release into a patch of the
//! RKE2 `HelmChart.spec.version`, by polling the chart registry and/or a release webhook.
//! Stateless; every decision is logged.

pub mod app;
pub mod charts;
pub mod filter;
pub mod poll;
pub mod registry;
pub mod signature;
pub mod update;
