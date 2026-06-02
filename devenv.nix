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
  # Socket-only (no listen_addresses) so it never collides with a system
  # Postgres on the TCP port; the socket lives in a per-project runtime dir.
  # The port only names the socket file here — set high/uncommon to dodge any
  # already-running Postgres.
  services.postgres = {
    enable = true;
    package = pkgs.postgresql_17;
    port = 54329;
    initialDatabases = [ { name = "ravn"; } ];
  };

  # NATS broker with JetStream, run as a managed process on a high/uncommon
  # port so it never collides with an already-running NATS on the default 4222.
  processes.nats.exec = "${pkgs.nats-server}/bin/nats-server --addr 127.0.0.1 --port 14222 --jetstream --store_dir ${config.env.DEVENV_STATE}/nats";

  env = {
    RUST_LOG = "info";
    NATS_URL = "nats://127.0.0.1:14222";
  };

  enterShell = ''
    # devenv runs Postgres on a unix socket in $PGHOST; derive DATABASE_URL
    # from the live socket so it always matches the running instance.
    export DATABASE_URL="postgresql:///ravn?host=$PGHOST&port=$PGPORT"
    echo "Ravn dev shell — rustc $(rustc --version | cut -d' ' -f2), node $(node --version)"
    echo "  devenv up      → start Postgres (socket) + NATS (:14222)"
    echo "  cargo build    → build the workspace"
  '';
}
