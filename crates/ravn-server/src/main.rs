//! `ravn-server` — the Ravn control plane.
//!
//! Skeleton entry point. The Axum API + OpenAPI (issue #23), NATS ingestion +
//! Postgres persistence (issue #24), and auth (issue #26) are wired in by their
//! respective issues.

fn main() {
    println!("ravn-server {} (ravn-core {})", env!("CARGO_PKG_VERSION"), ravn_core::VERSION);
}
