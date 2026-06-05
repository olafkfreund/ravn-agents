//! Generate a Ravn command-signing keypair.
//!
//! Prints `<private_b64> <public_b64>` on one line. The **private** key goes to
//! the control plane (`RAVN_COMMAND_KEY`); the **public** key is pinned on agents
//! and actuators (`services.ravn.agent.remediation.commandSigningPublicKey`).
//!
//!   cargo run --example keygen -p ravn-crypto

use ravn_crypto::{generate_signing_key, signing_key_to_b64, verifying_key_to_b64};

fn main() {
    let key = generate_signing_key();
    println!("{} {}", signing_key_to_b64(&key), verifying_key_to_b64(&key.verifying_key()));
}
