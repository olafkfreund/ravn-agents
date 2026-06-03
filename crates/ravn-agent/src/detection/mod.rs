//! Detection taps (epic #1).
//!
//! Each tap is deterministic: it decides *whether* something is worth reporting,
//! independent of any LLM, and emits normalized `ravn-core` events.

pub mod auth;
pub mod config_drift;
pub mod failed_unit;
pub mod journald;
