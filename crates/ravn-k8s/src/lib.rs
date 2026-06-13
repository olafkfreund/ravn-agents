//! Shared internals for the Ravn Kubernetes binaries.
//!
//! Three binaries build on this crate:
//! - `ravn-controller` (#55) — a cluster-wide Deployment that watches core/v1
//!   Events for workload failures ([`mapping`] + [`watcher`]);
//! - `ravn-node-agent` (#56) — a DaemonSet that watches its own Node's status
//!   conditions ([`node`]);
//! - `ravn-k8s-executor` (#146) — pulls signed [`ravn_core::CommandEnvelope`]s
//!   over NATS, re-verifies Ed25519 signatures, and executes typed K8s
//!   capabilities ([`executor`] + [`command_loop`]).
//!
//! All three reuse [`config`] (env-driven settings) and classify with
//! `ravn_core::kube_severity_for_reason` so signals stay consistent across
//! sources.

pub mod command_loop;
pub mod config;
pub mod executor;
pub mod http;
pub mod logs;
pub mod mapping;
pub mod nats;
pub mod node;
pub mod publish;
pub mod watcher;
