//! Shared types for Ravn.
//!
//! `ravn-core` is the contract between the agent and the control plane. The
//! normalized [`Event`] type defined here is the spine everything else hangs
//! off — see issue #14 for the full schema. This is a deliberate skeleton; the
//! real fields (severity, source, host, timestamp, category hints, payload)
//! land with that issue.

/// A normalized event emitted by an agent's detection layer.
///
/// Placeholder: the full schema is defined in issue #14.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Event;

/// Crate version, surfaced so the agent and server can report a build identity.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_populated() {
        assert!(!VERSION.is_empty());
    }
}
