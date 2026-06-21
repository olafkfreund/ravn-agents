{
  description = "Ravn — fleet detection agents with local inference and a control plane";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };

          # Pinned stable toolchain, shared by crane for all crates.
          rustToolchain = pkgs.rust-bin.stable.latest.default;
          craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

          # Rust/Cargo sources, plus data files cleanCargoSource would otherwise
          # strip (it keeps only *.rs/Cargo.*): test fixtures (#39) and the SQLx
          # migrations, which `sqlx::migrate!` embeds at build time (#24) — without
          # them the control plane starts with no schema. The ravn-eval harness
          # (#157) also reads a committed corpus + recordings + golden RESULTS.md
          # at test time, so keep those too or `nix flake check` fails.
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            name = "ravn-source";
            filter = path: type:
              (pkgs.lib.hasInfix "/tests/fixtures/" path)
              || (pkgs.lib.hasInfix "/migrations/" path)
              || (pkgs.lib.hasInfix "/ravn-eval/fixtures/" path)
              || (pkgs.lib.hasSuffix "/ravn-eval/RESULTS.md" path)
              # ravn-mcp's schema test (#156) validates against the committed
              # OpenAPI doc; without it the test silently skips in the sandbox.
              || (pkgs.lib.hasSuffix "/portal/openapi.json" path)
              || (craneLib.filterCargoSources path type);
          };

          # Native deps for the Rust crates:
          #   pkg-config + openssl -> TLS for NATS/HTTP clients
          #   systemd              -> sd-journal reader and D-Bus unit taps (M1)
          commonArgs = {
            inherit src;
            strictDeps = true;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.openssl pkgs.systemd ];
          };

          # Build all workspace dependencies once; cached across crate rebuilds.
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          # Per-crate packages (binaries only).
          ravn-agent = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "ravn-agent";
            cargoExtraArgs = "-p ravn-agent";
            doCheck = false;
            # The agent crate produces the `ravnd` daemon (epic #3).
            meta.mainProgram = "ravnd";
          });

          ravn-server = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "ravn-server";
            cargoExtraArgs = "-p ravn-server";
            doCheck = false;
            meta.mainProgram = "ravn-server";
          });

          # The privileged remediation executor (#113): the only privileged Ravn
          # component on a host. Packaged so the NixOS module can run it (#120).
          ravn-actuator = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "ravn-actuator";
            cargoExtraArgs = "-p ravn-actuator";
            doCheck = false;
            meta.mainProgram = "ravn-actuator";
          });

          # The Kubernetes binaries (#55/#56): controller + node-agent, both from
          # the ravn-k8s crate. One package, two bins.
          ravn-k8s = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "ravn-k8s";
            cargoExtraArgs = "-p ravn-k8s";
            doCheck = false;
            meta.mainProgram = "ravn-controller";
          });

          # The explanation-quality eval harness (#157). Scoring is deterministic
          # and the recorded runs are committed, so `ravn-eval` (no args) produces
          # a reproducible scored comparison table inside the Nix sandbox — no
          # model required. Its tests (incl. the RESULTS.md golden) run under
          # `workspace-test`.
          ravn-eval = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "ravn-eval";
            cargoExtraArgs = "-p ravn-eval";
            doCheck = false;
            meta.mainProgram = "ravn-eval";
          });

          # The MCP server (#156): a read-only-by-default Model Context Protocol
          # bridge to the control plane, shipped so MCP clients (Claude Code, etc.)
          # can inspect the fleet. Replaces the scripts/ prototypes.
          ravn-mcp = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "ravn-mcp";
            cargoExtraArgs = "-p ravn-mcp";
            doCheck = false;
            meta.mainProgram = "ravn-mcp";
          });

          # Reproducible OCI images (#37). Load with `docker load < $(nix build .#ravn-server-image --print-out-paths)`.
          ravn-server-image = pkgs.dockerTools.buildLayeredImage {
            name = "ravn-server";
            tag = "latest";
            contents = [ ravn-server pkgs.cacert ];
            config = {
              Entrypoint = [ "${ravn-server}/bin/ravn-server" ];
              ExposedPorts = { "8080/tcp" = { }; };
            };
          };

          ravn-agent-image = pkgs.dockerTools.buildLayeredImage {
            name = "ravn-agent";
            tag = "latest";
            contents = [ ravn-agent pkgs.cacert ];
            config.Entrypoint = [ "${ravn-agent}/bin/ravnd" ];
          };

          # One image carries both K8s binaries; the DaemonSet overrides the
          # entrypoint to run `ravn-node-agent`. `cacert` so rustls can verify
          # the control-plane / inference TLS chain.
          ravn-k8s-image = pkgs.dockerTools.buildLayeredImage {
            name = "ravn-k8s";
            tag = "latest";
            contents = [ ravn-k8s pkgs.cacert ];
            config.Entrypoint = [ "${ravn-k8s}/bin/ravn-controller" ];
          };

          # The MCP server image (#156). Speaks MCP over stdio, so it has no
          # exposed port; `cacert` lets rustls verify the control-plane TLS chain.
          ravn-mcp-image = pkgs.dockerTools.buildLayeredImage {
            name = "ravn-mcp";
            tag = "latest";
            contents = [ ravn-mcp pkgs.cacert ];
            config.Entrypoint = [ "${ravn-mcp}/bin/ravn-mcp" ];
          };
        in
        {
          packages = {
            inherit ravn-agent ravn-server ravn-k8s ravn-actuator ravn-eval ravn-mcp
              ravn-agent-image ravn-server-image ravn-k8s-image ravn-mcp-image;
            default = ravn-server;
          } // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
            # End-to-end VM test of the self-healing loop (#121): inject a failed
            # unit → propose → approve over the API → actuator heals → audited.
            # Deliberately NOT in `checks`: NixOS VM tests are slow under emulation,
            # so it runs in its own CI job (.github/workflows/vmtest.yml) rather than
            # gating every PR via `nix flake check`.
            remediation-e2e = import ./nixos/tests/remediation.nix { inherit self pkgs; };
            # Air-gapped heal-loop VM test (#153): same signed heal path but with
            # outbound networking blocked at the firewall level and RAVN_AIRGAPPED=1
            # set.  Runs without a real model (inference is disabled) so it fits in
            # the standard CI RAM budget.  Kept out of `checks` for the same reason
            # as the remediation e2e above.
            airgapped-e2e = import ./nixos/tests/airgapped.nix { inherit self pkgs; };
          };

          checks = {
            inherit ravn-agent ravn-server ravn-k8s ravn-actuator ravn-eval ravn-mcp;

            # Whole-workspace clippy and tests gate `nix flake check`.
            workspace-clippy = craneLib.cargoClippy (commonArgs // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            });

            workspace-test = craneLib.cargoTest (commonArgs // {
              inherit cargoArtifacts;
            });
          };

          # Lightweight shell for CI / `nix develop`. Day-to-day dev uses devenv.
          devShells.default = craneLib.devShell {
            inputsFrom = [ ravn-agent ravn-server ];
          };
        }) // {
      # System-independent outputs.

      # Adds the Ravn binaries to a package set, so the NixOS modules can
      # default their `package` option to `pkgs.ravn-agent` / `pkgs.ravn-server`.
      overlays.default = final: _prev: {
        ravn-agent = self.packages.${final.stdenv.hostPlatform.system}.ravn-agent;
        ravn-server = self.packages.${final.stdenv.hostPlatform.system}.ravn-server;
        ravn-actuator = self.packages.${final.stdenv.hostPlatform.system}.ravn-actuator;
      };

      nixosModules = {
        # `services.ravn.agent` (#35). Imports the implementation and applies
        # the overlay so the package default resolves out of the box.
        agent = { ... }: {
          imports = [ ./nixos/modules/agent.nix ];
          nixpkgs.overlays = [ self.overlays.default ];
        };
        # `services.ravn.controlPlane` (#36).
        controlPlane = { ... }: {
          imports = [ ./nixos/modules/control-plane.nix ];
          nixpkgs.overlays = [ self.overlays.default ];
        };
        default = self.nixosModules.agent;
      };

      # Canonical single-machine example. Replicate the node (agent2, agent3, …)
      # to grow into a fleet. Built as a container profile so it evaluates cheaply.
      nixosConfigurations.demo-agent = nixpkgs.lib.nixosSystem {
        modules = [
          self.nixosModules.agent
          ({ ... }: {
            nixpkgs.hostPlatform = "x86_64-linux";
            boot.isContainer = true;
            system.stateVersion = "25.05";
            networking.hostName = "demo-agent";

            services.ravn.agent = {
              enable = true;
              server.url = "nats://control.example.com:4222";
              enrollment.bootstrapTokenFile = "/run/secrets/ravn-bootstrap-token";
              detection.configDrift.paths = [ "/etc/nixos" ];
              inference.model.path = "/var/lib/ravn/models/qwen3-1.7b-q4_k_m.gguf";
            };
          })
        ];
      };

      # Canonical control-plane host: control plane + local NATS + Postgres.
      nixosConfigurations.demo-control-plane = nixpkgs.lib.nixosSystem {
        modules = [
          self.nixosModules.controlPlane
          ({ ... }: {
            nixpkgs.hostPlatform = "x86_64-linux";
            boot.isContainer = true;
            system.stateVersion = "25.05";
            networking.hostName = "demo-control-plane";

            services.ravn.controlPlane = {
              enable = true;
              bind = "0.0.0.0:8080";
            };
          })
        ];
      };

      # ── Air-gapped configurations (#153) ──────────────────────────────────
      # These two configurations are the canonical reference for deploying Ravn
      # in a network-isolated environment.  See nixos/configurations/airgapped.nix
      # and docs/airgapped-install.md for full documentation.
      #
      # They evaluate as container profiles (no bootloader/disk layout) so
      # `nix flake check` and CI can evaluate them cheaply.
      #
      # To deploy on bare metal: remove `boot.isContainer = true` and add
      # your hardware-specific boot and filesystem configuration.
      nixosConfigurations.airgap-control-plane = nixpkgs.lib.nixosSystem {
        modules = [
          self.nixosModules.controlPlane
          ({ ... }: {
            nixpkgs.hostPlatform = "x86_64-linux";
            boot.isContainer = true;
            system.stateVersion = "25.05";
            networking.hostName = "ravn-control";

            services.ravn.controlPlane = {
              enable = true;
              # Bind to all interfaces; put a reverse proxy with mTLS in front.
              bind = "0.0.0.0:8080";
              database.createLocally = true;
              nats.createLocally = true;
              enrollment = {
                enable = true;
                # CA cert is public — safe to reference from the store.
                caCertFile = "/etc/ravn/ca.crt";
                # Key and token arrive as systemd credentials; never in the store.
                caKeyFile = "/run/secrets/ravn-ca-key";
                bootstrapTokenFile = "/run/secrets/ravn-bootstrap-token";
                certTtlDays = 365;
              };
            };

            # Block outbound JWKS URL fetches; all OIDC docs must be local files.
            systemd.services.ravn-server.environment.RAVN_AIRGAPPED = "1";

            # Deny all forward routing — this host has no internet path.
            networking.firewall.enable = true;
            networking.firewall.extraCommands = "iptables -P FORWARD DROP";
            # Allow agents on the internal network to reach the API.
            networking.firewall.allowedTCPPorts = [ 8080 ];
          })
        ];
      };

      nixosConfigurations.airgap-agent = nixpkgs.lib.nixosSystem {
        modules = [
          self.nixosModules.agent
          ({ ... }: {
            nixpkgs.hostPlatform = "x86_64-linux";
            boot.isContainer = true;
            system.stateVersion = "25.05";
            networking.hostName = "ravn-node-01";

            services.ravn.agent = {
              enable = true;
              # Internal NATS — never touches the internet.
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
                # Use `path`, not `url` — the model must be pre-copied to the host.
                # See docs/airgapped-install.md for the transfer procedure.
                model = {
                  name = "qwen3-1.7b-q4_k_m";
                  path = "/var/lib/ravn/models/qwen3-1.7b-q4_k_m.gguf";
                  # url is intentionally absent: setting it would trigger a
                  # pkgs.fetchurl download on the build host.
                };
                host = "127.0.0.1";
                port = 18181;
              };
              remediation = {
                enable = true;
                # Replace with the actual public key from your control plane.
                commandSigningPublicKey = "REPLACE_WITH_BASE64_ED25519_PUBKEY";
                pollSecs = 10;
              };
            };

            networking.firewall.enable = true;
            networking.firewall.extraCommands = "iptables -P FORWARD DROP";
          })
        ];
      };
    };
}
