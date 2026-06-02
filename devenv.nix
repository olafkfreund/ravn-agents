{ pkgs, config, ... }:

{
  # Ravn development shell.
  # Activate with `direnv allow`, or enter manually with `devenv shell`.
  # Background services (Postgres, NATS) start with `devenv up`.

  # Rust toolchain (stable) with the components we need across the workspace.
  languages.rust = {
    enable = true;
    channel = "stable";
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" ];
  };

  # Frontend toolchain for the portal (crates 5/6).
  languages.javascript = {
    enable = true;
    package = pkgs.nodejs_22;
    pnpm.enable = true;
  };

  packages = with pkgs; [
    # Messaging — agent transport and control-plane ingestion (NATS).
    natscli
    nats-server

    # Local inference for the agent (llama.cpp / llama-server).
    llama-cpp

    # Native build dependencies for the Rust crates:
    #   pkg-config + openssl  -> TLS for NATS/HTTP clients
    #   systemd               -> sd-journal reader and D-Bus unit taps (M1 detection)
    pkg-config
    openssl
    systemd

    # Database tooling (server uses SQLx against Postgres).
    postgresql_17
    sqlx-cli

    # Nix authoring tooling per project standards.
    nixd
    statix
    deadnix
    nixpkgs-fmt
  ];

  # Control-plane Postgres, managed by devenv (`devenv up`).
  services.postgres = {
    enable = true;
    package = pkgs.postgresql_17;
    listen_addresses = "127.0.0.1";
    initialDatabases = [ { name = "ravn"; } ];
  };

  # NATS broker with JetStream, run as a managed process.
  processes.nats.exec = "${pkgs.nats-server}/bin/nats-server --jetstream --store_dir ${config.env.DEVENV_STATE}/nats";

  env = {
    # Convenience defaults wired to the devenv Postgres socket.
    DATABASE_URL = "postgres:///ravn?host=${config.env.DEVENV_STATE}/postgres";
    RUST_LOG = "info";
    NATS_URL = "nats://127.0.0.1:4222";
  };

  enterShell = ''
    echo "Ravn dev shell — rustc $(rustc --version | cut -d' ' -f2), node $(node --version)"
    echo "  devenv up      → start Postgres + NATS"
    echo "  cargo build    → build the workspace"
  '';
}
