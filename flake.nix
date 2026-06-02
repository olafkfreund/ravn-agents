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
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # Pinned stable toolchain, shared by crane for all crates.
        rustToolchain = pkgs.rust-bin.stable.latest.default;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        src = craneLib.cleanCargoSource ./.;

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
      in
      {
        packages = {
          inherit ravn-agent ravn-server;
          default = ravn-server;
        };

        checks = {
          inherit ravn-agent ravn-server;

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
      };

      nixosModules = {
        # `services.ravn.agent` (#35). Imports the implementation and applies
        # the overlay so the package default resolves out of the box.
        agent = { ... }: {
          imports = [ ./nixos/modules/agent.nix ];
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
            };
          })
        ];
      };
    };
}
