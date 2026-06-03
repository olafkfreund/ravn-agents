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

    # Kubernetes dev cluster + tooling for the K8s epic (#53). k3d runs k3s
    # inside Docker; kubectl/k9s/helm drive it. The cluster is created on an
    # uncommon API port via `k3d-up` (see scripts below).
    k3d
    kubectl
    k9s
    kubernetes-helm

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
    initialDatabases = [{ name = "ravn"; }];
  };

  # NATS broker with JetStream, run as a managed process on a high/uncommon
  # port so it never collides with an already-running NATS on the default 4222.
  processes.nats.exec = "${pkgs.nats-server}/bin/nats-server --addr 127.0.0.1 --port 14222 --jetstream --store_dir ${config.env.DEVENV_STATE}/nats";

  env = {
    RUST_LOG = "info";
    NATS_URL = "nats://127.0.0.1:14222";
    # Project-local kubeconfig so the k3d cluster never clobbers the user's
    # ~/.kube/config (they likely run other clusters).
    KUBECONFIG = "${config.env.DEVENV_STATE}/kubeconfig";
  };

  # k3d cluster lifecycle (#53). The cluster API binds an uncommon host port
  # (16443) to avoid colliding with anything on the default 6443.
  scripts.k3d-up.exec = ''
    set -euo pipefail
    cfg="$DEVENV_ROOT/k8s/k3d/cluster.yaml"
    if k3d cluster list ravn-dev >/dev/null 2>&1; then
      echo "cluster 'ravn-dev' already exists"
    else
      echo "creating k3d cluster 'ravn-dev' (API on 127.0.0.1:16443)…"
      k3d cluster create --config "$cfg" \
        --kubeconfig-update-default=false --kubeconfig-switch-context=false
    fi
    k3d kubeconfig write ravn-dev --output "$KUBECONFIG" >/dev/null
    kubectl apply -f "$DEVENV_ROOT/k8s/test-workloads.yaml"
    echo "KUBECONFIG=$KUBECONFIG"
    kubectl get pods -n ravn-test -o wide || true
  '';
  scripts.k3d-down.exec = ''k3d cluster delete ravn-dev'';
  scripts.k3d-status.exec = ''
    set -euo pipefail
    kubectl get nodes
    echo
    kubectl get pods -n ravn-test -o wide
  '';

  enterShell = ''
    # devenv runs Postgres on a unix socket in $PGHOST; derive DATABASE_URL
    # from the live socket so it always matches the running instance.
    export DATABASE_URL="postgresql:///ravn?host=$PGHOST&port=$PGPORT"
    echo "Ravn dev shell — rustc $(rustc --version | cut -d' ' -f2), node $(node --version)"
    echo "  devenv up      → start Postgres (socket) + NATS (:14222)"
    echo "  cargo build    → build the workspace"
    echo "  k3d-up         → create the k3d test cluster + 2 pods (API :16443)"
    echo "  k3d-status     → nodes + test pods   |   k3d-down → delete cluster"
  '';
}
