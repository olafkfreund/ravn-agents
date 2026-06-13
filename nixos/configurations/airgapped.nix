# Air-gapped Ravn deployment — NixOS configuration example (issue #153).
#
# This configuration is the canonical reference for deploying Ravn on a host
# that has no outbound internet access.  Every default path in Ravn is already
# offline-compatible *except* for:
#
#   1. JWKS URL fetch (`RAVN_OIDC_JWKS_URL` / `RAVN_INGEST_OIDC_JWKS_URL`):
#      gated by RAVN_AIRGAPPED=1 which rejects the URL variant at startup.
#   2. Model download via `inference.model.url` in the agent module:
#      the `path` option is used here instead (GGUF pre-copied to the host).
#   3. `pkgs.cacert` in OCI images: not needed here (no HTTPS to external CAs).
#
# HOW TO USE THIS FILE
# --------------------
# 1.  Copy the Nix closure to the air-gapped host:
#       nix copy --to "ssh://TARGET" .#nixosConfigurations.airgap-control-plane.config.system.build.toplevel
#       nix copy --to "ssh://TARGET" .#nixosConfigurations.airgap-agent.config.system.build.toplevel
#
# 2.  Copy the model weight file to the agent host:
#       scp /local/models/qwen3-1.7b-q4_k_m.gguf TARGET:/var/lib/ravn/models/
#
# 3.  Generate and deploy secrets out-of-band (see the air-gapped install doc).
#
# 4.  On the target: nixos-rebuild switch --flake .#airgap-control-plane
#
# This example uses boot.isContainer = true so it evaluates cheaply in CI
# without a full bootloader/disk layout.  Remove that option for bare-metal.

{ self, nixpkgs }:

let
  # ── shared helpers ────────────────────────────────────────────────────────
  baseModule = { pkgs, lib, ... }: {
    nixpkgs.hostPlatform = "x86_64-linux";
    boot.isContainer = true;
    system.stateVersion = "25.05";

    # Completely block outbound traffic at the kernel firewall level.
    # Internal services (NATS, Postgres, loopback inference) are AF_UNIX or
    # 127.0.0.1 and are not affected.
    networking.firewall = {
      enable = true;
      # Only allow inbound traffic to the control-plane API port from the
      # internal management network (adjust to your VLAN).
      allowedTCPPorts = [ ];
    };

    # Disable NixOS channel update fetches — there is no internet here.
    # Updates are delivered by re-running `nix copy` from a build host.
    system.autoUpgrade.enable = false;
    nix.settings.auto-optimise-store = true;
    nix.gc = {
      automatic = true;
      dates = "weekly";
      options = "--delete-older-than 14d";
    };
  };

in
{
  # ── control plane ─────────────────────────────────────────────────────────
  #
  # nixosConfigurations.airgap-control-plane — the central broker.
  # Runs ravn-server, NATS (JetStream), and PostgreSQL, all loopback-only.
  # A reverse proxy (e.g. nginx) with mTLS sits in front for agent access.
  airgap-control-plane = nixpkgs.lib.nixosSystem {
    modules = [
      self.nixosModules.controlPlane
      baseModule
      ({ config, lib, pkgs, ... }: {
        networking.hostName = "ravn-control";

        services.ravn.controlPlane = {
          enable = true;
          # Bind to all interfaces so agents on the internal network can reach it.
          # Place nginx with mTLS termination in front for production.
          bind = "0.0.0.0:8080";

          database.createLocally = true;
          nats.createLocally = true;

          enrollment = {
            enable = true;
            # CA certificate (public): safe in the Nix store.
            caCertFile = "/etc/ravn/ca.crt";
            # CA private key: delivered at runtime as a systemd credential,
            # never stored in the Nix store or process environment.
            caKeyFile = "/run/secrets/ravn-ca-key";
            bootstrapTokenFile = "/run/secrets/ravn-bootstrap-token";
            certTtlDays = 365;
          };
        };

        # Air-gapped mode: rejects any JWKS URL fetch at startup.
        # All OIDC documents must be supplied as local files.
        systemd.services.ravn-server.environment.RAVN_AIRGAPPED = "1";

        # Portal user auth (OIDC) — supply the JWKS from a local file that was
        # exported from your internal IdP (e.g. Keycloak) and scp'd here.
        # Leave these commented out if you use only bearer-token auth.
        #
        # systemd.services.ravn-server.environment = {
        #   RAVN_OIDC_ISSUER    = "https://idp.internal.example.com/realms/ravn";
        #   RAVN_OIDC_AUDIENCE  = "ravn-portal";
        #   RAVN_OIDC_CLIENT_ID = "ravn-portal";
        #   RAVN_OIDC_JWKS_FILE = "/etc/ravn/oidc-jwks.json";  # NOT a URL
        # };
        #
        # Ingest auth (Kubernetes ServiceAccount OIDC) — same approach:
        # copy the cluster's JWKS document and reference it as a file.
        #
        # systemd.services.ravn-server.environment = {
        #   RAVN_INGEST_OIDC_ISSUER   = "https://kube.internal.example.com";
        #   RAVN_INGEST_OIDC_JWKS_FILE = "/etc/ravn/kube-jwks.json";
        # };

        # Internal CA certificate: reference only (public data, may be in store).
        environment.etc."ravn/ca.crt".source = /etc/ravn/ca.crt;

        # Secrets arrive via agenix or a secrets manager and are NOT in the
        # Nix store.  See the air-gapped install doc for secret provisioning.

        # Firewall: allow agents on the internal network to reach the API.
        networking.firewall.allowedTCPPorts = [ 8080 ];

        # No internet access at all — explicitly deny forward routing.
        networking.firewall.extraCommands = ''
          iptables -P FORWARD DROP
        '';
      })
    ];
  };

  # ── agent ─────────────────────────────────────────────────────────────────
  #
  # nixosConfigurations.airgap-agent — a monitored fleet node.
  # Each fleet member gets one of these.  Replication: copy the module and
  # change `networking.hostName`.
  airgap-agent = nixpkgs.lib.nixosSystem {
    modules = [
      self.nixosModules.agent
      baseModule
      ({ config, lib, pkgs, ... }: {
        networking.hostName = "ravn-node-01";

        services.ravn.agent = {
          enable = true;

          # Internal NATS endpoint — never reaches the internet.
          server.url = "nats://ravn-control.internal.example.com:4222";

          enrollment = {
            endpoint = "https://ravn-control.internal.example.com:8080";
            bootstrapTokenFile = "/run/secrets/ravn-bootstrap-token";
          };

          detection = {
            journald.enable = true;
            failedUnits.enable = true;
            configDrift.paths = [ "/etc/nixos" "/etc/ssh/sshd_config" ];
            auth.enable = true;
            updates.enable = true;
          };

          inference = {
            enable = true;

            # CRITICAL: use `path`, not `url`.  The model must be pre-copied to
            # the host before first boot (see air-gapped install doc).
            model = {
              name = "qwen3-1.7b-q4_k_m";
              # The GGUF file must exist at this path on the host.
              path = "/var/lib/ravn/models/qwen3-1.7b-q4_k_m.gguf";
              # Do NOT set `url` — it would trigger a download on first boot.
            };

            # Loopback-only: the inference service never touches the network.
            host = "127.0.0.1";
            port = 18181;
            contextSize = 4096;
            # Use all physical cores; no SMT oversubscription.
            threads = 0;
            cpuQuota = "200%";
            memoryMax = "2G";
          };

          remediation = {
            enable = true;
            # Public key: exported from the control plane at enrollment time.
            commandSigningPublicKey = "REPLACE_WITH_BASE64_ED25519_PUBKEY";
            pollSecs = 10;
          };
        };

        # No outbound traffic from agents either.
        networking.firewall.extraCommands = ''
          iptables -P FORWARD DROP
        '';
      })
    ];
  };
}
