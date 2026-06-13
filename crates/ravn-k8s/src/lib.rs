//! Shared internals for the Ravn Kubernetes binaries.
//!
//! Two binaries build on this crate:
//! - `ravn-controller` (#55) — a cluster-wide Deployment that watches core/v1
//!   Events for workload failures ([`mapping`] + [`watcher`]);
//! - `ravn-node-agent` (#56) — a DaemonSet that watches its own Node's status
//!   conditions ([`node`]).
//!
//! Both reuse [`config`] (env-driven settings) and [`nats`] (the publisher),
//! and classify with `ravn_core::kube_severity_for_reason` so signals stay
//! consistent across sources.

pub mod config;
pub mod http;
pub mod logs;
pub mod mapping;
pub mod nats;
pub mod node;
pub mod publish;
pub mod watcher;
