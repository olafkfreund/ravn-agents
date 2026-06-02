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
        });

        ravn-server = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "ravn-server";
          cargoExtraArgs = "-p ravn-server";
          doCheck = false;
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
      });
}
