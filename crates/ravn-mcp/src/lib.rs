//! `ravn-mcp` — the supported Ravn [Model Context Protocol][mcp] server (#156).
//!
//! This crate replaces the two divergent `scripts/ravn-mcp.js` /
//! `scripts/ravn_mcp_server.py` prototypes with one versioned, tested component
//! that ships with releases. It exposes the Ravn control plane to MCP clients
//! (Claude Code and others) as a small set of tools, speaking JSON-RPC 2.0 over
//! stdio.
//!
//! # Doctrine: read-only by default
//!
//! The five **read-only** tools are always available:
//!
//! - `list_agents` — registered agents, status, labels
//! - `list_recent_events` — recent deterministic detections
//! - `get_event` — one event by id
//! - `get_topology` — the fleet grouped for the topology view
//! - `list_remediation_proposals` — proposals awaiting a decision
//!
//! The two **mutating** tools — `approve_remediation` and `reject_remediation` —
//! are only registered when the server is started with `--allow-mutations` and
//! an admin-scoped token. Even then they merely call the same control-plane
//! endpoints a human would click in the portal: a signed, human-attributed
//! command is *enqueued* for the agent to pull. The MCP server never executes a
//! remediation and never auto-approves. This mirrors the broader Ravn doctrine
//! (#112–#117): actions are deterministic, human- or policy-approved, and
//! independently re-verified by the privileged actuator.
//!
//! # Layout
//!
//! - [`config`] — flag/env parsing
//! - [`client`] — the control-plane HTTP client
//! - [`protocol`] — JSON-RPC 2.0 + MCP wire types
//! - [`tools`] — tool definitions, the read-only/gated split, and dispatch
//! - [`server`] — the request [`server::Handler`] and the stdio loop
//!
//! [mcp]: https://modelcontextprotocol.io

pub mod client;
pub mod config;
pub mod protocol;
pub mod server;
pub mod tools;

pub use client::ControlPlaneClient;
pub use config::Config;
pub use server::{serve_stdio, Handler};
