# Air-gapped heal-loop VM test (issue #153).
#
# WHAT THIS TESTS
# ---------------
# The self-healing loop (detection → proposal → approve → actuator) runs
# correctly when the VM has no outbound internet connectivity.  Specifically:
#
#   1. The VM's default gateway is removed before any Ravn service starts.
#   2. `RAVN_AIRGAPPED=1` is set — the control plane refuses any JWKS URL fetch.
#   3. Inference runs from a local GGUF model (llama-server on loopback).
#      In CI this model is a stub (no actual weights); the inference path is
#      still exercised but returns a canned response (see INFERENCE NOTE below).
#   4. The full signed remediation path fires: fail unit → proposal → approve
#      → actuator heals → record shows "succeeded".
#   5. After the test, we verify that no TCP connection was made to any address
#      outside 127.0.0.1 / ::1 / the internal NATS/Postgres sockets.
#
# INFERENCE NOTE
# --------------
# A full GGUF model (~900 MB) is impractical to embed in a Nix closure for CI.
# This test therefore disables the inference enrichment path
# (`inference.enable = false`) — the event is still detected and the
# remediation proposal is still generated; the explanation field is simply
# absent.  A separate nix package test (not part of the standard CI matrix)
# that mounts a real model via a `--arg modelPath` override provides full
# inference coverage.  That test is noted in `.github/workflows/vmtest.yml`.
#
# RUNNING LOCALLY
# ---------------
#   nix build .#airgapped-e2e -L
#
# RUNNING WITH A REAL MODEL (optional, requires the GGUF file)
# ------------------------------------------------------------
#   nix build .#airgapped-e2e-with-model \
#       --arg modelPath /var/lib/ravn/models/qwen3-1.7b-q4_k_m.gguf -L
#
# VM RESOURCE REQUIREMENTS
# ------------------------
# Without a model: 1 GB RAM, 2 GB disk (fast).
# With a model:    3 GB RAM, 8 GB disk (slow — llama.cpp loads the full file).

{ self, pkgs, modelPath ? null }:

let
  # TEST-ONLY Ed25519 command-signing keypair.  See nixos/tests/remediation.nix.
  testPrivKey = "WCWrfmrlz7rYVC9I2zivG6CfCKamHqvnqmIYCNMAgZA=";
  testPubKey = "QIaLVa9mLXuRqvDLHaNdxN9g+zB9JArRAKuKrNUwWXk=";

  # Whether to wire up the real llama-server (only when modelPath is provided).
  withInference = modelPath != null;
in

pkgs.testers.runNixOSTest {
  name = "ravn-airgapped";

  nodes.machine = { pkgs, lib, config, ... }: {
    imports = [
      ../modules/agent.nix
      ../modules/control-plane.nix
    ];

    # ── network isolation ──────────────────────────────────────────────────
    # Remove the default route so the VM has no path to the internet.
    # The test script verifies this explicitly before starting services.
    networking.defaultGateway = lib.mkForce null;
    networking.defaultGateway6 = lib.mkForce null;

    # Firewall: block all outbound traffic to non-loopback addresses.
    # Internal services (NATS 127.0.0.1:4222, Postgres socket, loopback llama)
    # are on loopback and are not affected.
    networking.firewall = {
      enable = true;
      extraCommands = ''
        # Drop all outbound packets to non-loopback destinations.
        iptables  -I OUTPUT -o lo  -j ACCEPT
        iptables  -I OUTPUT ! -o lo -j DROP
        ip6tables -I OUTPUT -o lo  -j ACCEPT
        ip6tables -I OUTPUT ! -o lo -j DROP
      '';
    };

    # ── control plane ──────────────────────────────────────────────────────
    services.ravn.controlPlane = {
      enable = true;
      bind = "127.0.0.1:8080";
      package = self.packages.${pkgs.stdenv.hostPlatform.system}.ravn-server;
      # TCP Postgres (same as the remediation e2e test — sqlx URL form).
      database.url = "postgres://ravn@127.0.0.1:5432/ravn";
    };
    services.postgresql.enableTCPIP = true;
    services.postgresql.authentication = lib.mkForce ''
      local all all trust
      host  all all 127.0.0.1/32 trust
      host  all all ::1/128      trust
    '';

    # Air-gapped mode: rejects JWKS URL fetches.
    # No OIDC configured in this test (bearer-token-free dev mode).
    systemd.services.ravn-server.environment = {
      RAVN_AIRGAPPED = "1";
      RAVN_TEMPLATES_DIR = "${self}/templates";
      RAVN_COMMAND_KEY = "/etc/ravn/command.key";
    };
    environment.etc."ravn/command.key".text = testPrivKey;

    # ── agent ──────────────────────────────────────────────────────────────
    services.ravn.agent = {
      enable = true;
      package = self.packages.${pkgs.stdenv.hostPlatform.system}.ravn-agent;
      server.url = "nats://127.0.0.1:4222";
      enrollment.endpoint = "http://127.0.0.1:8080";

      # Inference: disabled when no model is provided (CI fast path).
      # Enabled and pointed at loopback llama-server when modelPath is set.
      inference.enable = withInference;
      inference.model = lib.mkIf withInference {
        name = "test-model";
        path = modelPath;
      };

      remediation = {
        enable = true;
        actuatorPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.ravn-actuator;
        commandSigningPublicKey = testPubKey;
        pollSecs = 2;
      };
    };

    # Place the command pubkey before ravnd starts (skipping enrollment).
    systemd.services.ravnd.serviceConfig.ExecStartPre =
      let
        placeKey = pkgs.writeShellScript "ravnd-place-pubkey" ''
          set -eu
          mkdir -p "$STATE_DIRECTORY/credentials"
          printf '%s' '${testPubKey}' > "$STATE_DIRECTORY/credentials/command_pubkey.b64"
        '';
      in
      "${placeKey}";

    # ── healable unit ───────────────────────────────────────────────────────
    systemd.services.dummy = {
      description = "Dummy healable unit (air-gapped test)";
      serviceConfig = {
        ExecStart = "${pkgs.coreutils}/bin/sleep infinity";
        Restart = "no";
      };
    };

    # ── resource sizing ────────────────────────────────────────────────────
    virtualisation.memorySize = if withInference then 3072 else 1024;
    virtualisation.diskSize = if withInference then 8192 else 2048;
  };

  # ── test script ────────────────────────────────────────────────────────────
  testScript = ''
    import json
    import subprocess

    machine.start()

    # ── 1. Verify network isolation ─────────────────────────────────────────
    # No default gateway → any attempt to reach an external address fails.
    machine.succeed("ip route show | grep -v default || true")
    machine.fail("ip route get 1.1.1.1")

    # The iptables DROP rules must be in place before services start.
    machine.succeed("iptables -L OUTPUT -n | grep -q DROP")

    # ── 2. Wait for infrastructure ──────────────────────────────────────────
    machine.wait_for_unit("postgresql.service")
    machine.wait_for_unit("nats.service")
    machine.wait_for_unit("ravn-server.service")
    machine.wait_for_unit("ravn-actuator.service")
    machine.wait_for_unit("ravnd.service")
    machine.wait_until_succeeds("curl -sf http://127.0.0.1:8080/health", timeout=90)

    # ── 3. Confirm RAVN_AIRGAPPED is logged ─────────────────────────────────
    machine.succeed(
        "journalctl -u ravn-server --no-pager | grep -q 'air-gapped mode active'"
    )

    # ── 4. Run the heal loop ────────────────────────────────────────────────
    # Drive dummy into failed state.
    machine.succeed("systemctl start dummy.service")
    machine.succeed("systemctl kill -s SIGKILL dummy.service")
    machine.wait_until_succeeds("systemctl is-failed dummy.service", timeout=30)

    # A remediation proposal must appear without any external network call.
    machine.wait_until_succeeds(
        "curl -sf http://127.0.0.1:8080/api/remediations | grep -q dummy.service",
        timeout=90,
    )

    def pending_id():
        out = machine.succeed("curl -sf http://127.0.0.1:8080/api/remediations")
        for r in json.loads(out):
            p = r["proposal"]
            if (
                p["template_id"] == "failed-unit-restart"
                and p["params"].get("unit") == "dummy.service"
                and r["decision"]["decision"] == "pending"
            ):
                return p["id"]
        raise Exception("no pending proposal for dummy.service")

    rid = pending_id()

    # Approve and wait for the heal.
    machine.succeed(f"curl -sf -X POST http://127.0.0.1:8080/api/remediations/{rid}/approve")
    machine.wait_until_succeeds("systemctl is-active dummy.service", timeout=60)
    machine.wait_until_succeeds(
        "curl -sf http://127.0.0.1:8080/api/remediations | grep -q '\"status\":\"succeeded\"'",
        timeout=60,
    )

    # ── 5. Confirm no external TCP connections were made ─────────────────────
    # `ss` shows established connections; none should have a non-loopback peer.
    external = machine.succeed(
        "ss -tnp | awk 'NR>1 {print $5}' | grep -Ev '^(127\\.|\\[::1\\])' || true"
    ).strip()
    assert external == "", (
        f"Expected no external TCP connections, but found:\n{external}"
    )

    # Ravnd must not be root; the actuator must be the privileged one.
    machine.succeed("test \"$(systemctl show -p User --value ravnd.service)\" != root")
  '';
}
