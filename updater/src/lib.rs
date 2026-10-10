//! enzarb-updater: turns a published `enzarb-helm` GitHub release into a patch of the
//! RKE2 `HelmChart.spec.version`. Stateless; every decision is logged.

pub mod app;
pub mod charts;
pub mod filter;
pub mod registry;
pub mod signature;
