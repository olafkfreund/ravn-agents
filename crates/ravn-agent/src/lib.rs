//! Shared library surface of the Ravn agent.
//!
//! The `ravnd` binary keeps its operational modules (config, detection,
//! transport, buffer) private; only the self-contained [`inference`] module is
//! exposed here so the model eval harness (#38) can benchmark the *real* prompt
//! and response parsing against the fixture set (#39) without duplicating them.

pub mod inference;
