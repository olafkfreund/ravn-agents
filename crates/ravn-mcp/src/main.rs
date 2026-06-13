//! `ravn-mcp` binary entrypoint.
//!
//! Parses flags/env, builds the control-plane client, and runs the MCP stdio
//! loop. All logging goes to **stderr** so stdout carries only JSON-RPC frames —
//! mixing log lines into stdout would corrupt the protocol stream.

use std::process::ExitCode;

use ravn_mcp::config::{help_text, Config};
use ravn_mcp::{serve_stdio, ControlPlaneClient, Handler};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::parse(std::env::args().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => {
            // --help
            println!("{}", help_text());
            return ExitCode::SUCCESS;
        }
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::FAILURE;
        }
    };

    init_tracing(&config.log);

    if config.allow_mutations && config.token.is_none() {
        tracing::warn!(
            "--allow-mutations set but no --token/RAVN_MCP_TOKEN supplied; the control plane will \
             reject mutating calls unless its own auth is disabled"
        );
    }
    tracing::info!(
        url = %config.base_url,
        allow_mutations = config.allow_mutations,
        authenticated = config.token.is_some(),
        "ravn-mcp starting (stdio)"
    );

    let client = match ControlPlaneClient::new(config.base_url, config.token) {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "failed to build control-plane client");
            return ExitCode::FAILURE;
        }
    };

    let handler = Handler::new(client, config.allow_mutations);
    if let Err(error) = serve_stdio(handler).await {
        tracing::error!(%error, "MCP stdio loop ended with error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Initialise tracing to **stderr** with the configured filter. `RUST_LOG`, if
/// set, is already folded into `config.log` by the parser.
fn init_tracing(filter: &str) {
    let env_filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .init();
}
