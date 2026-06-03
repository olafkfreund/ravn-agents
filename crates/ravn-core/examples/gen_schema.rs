//! Writes the JSON Schema for the `Message` envelope to
//! `crates/ravn-core/schema/message.schema.json`.
//!
//! Run with: `cargo run --example gen_schema -p ravn-core`

use std::{fs, path::Path};

fn main() -> std::io::Result<()> {
    let schema = ravn_core::message_schema();
    let json = serde_json::to_string_pretty(&schema).expect("schema serializes");

    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("schema");
    fs::create_dir_all(&out_dir)?;
    let out = out_dir.join("message.schema.json");
    fs::write(&out, json + "\n")?;

    println!("wrote {}", out.display());
    Ok(())
}
