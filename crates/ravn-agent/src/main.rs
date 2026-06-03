//! `ravnd` — the Ravn agent daemon.
//!
//! Skeleton entry point. Detection taps (epic #1), local inference (epic #2),
//! and transport (epic #3) are wired in by their respective issues.

fn main() {
    println!("ravnd {} (ravn-core {})", env!("CARGO_PKG_VERSION"), ravn_core::VERSION);
}
