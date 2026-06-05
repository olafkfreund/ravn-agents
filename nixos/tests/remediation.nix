# End-to-end NixOS VM test for the supervised self-healing loop (#121).
#
# One machine runs the whole stack — control plane (ravn-server + Postgres +
# NATS), the agent (ravnd), and the privileged actuator — with remediation
# enabled. The test fails a systemd unit, waits for a proposal, approves it over
# the API, and asserts the actuator heals the unit and the record records success.
#
# To keep the test self-contained, it skips enrollment/mTLS and API auth (both
# orthogonal to the remediation path) and uses a fixed TEST-ONLY signing keypair
# so the server's signature and the actuator's verify key match. The whole signed
# path — sign → enqueue → pull → verify → privileged execute → result — is real.
{ self, pkgs }:

let
  # TEST-ONLY Ed25519 command-signing keypair (generated with
  # `cargo run --example keygen -p ravn-crypto`). Not used anywhere real.
  testPrivKey = "WCWrfmrlz7rYVC9I2zivG6CfCKamHqvnqmIYCNMAgZA=";
  testPubKey = "QIaLVa9mLXuRqvDLHaNdxN9g+zB9JArRAKuKrNUwWXk=";
in
pkgs.testers.runNixOSTest {
  name = "ravn-remediation";

  nodes.machine = { pkgs, lib, ... }: {
    # Import the module implementations directly and pass the Ravn packages
    # explicitly — the nixosModules wrappers use an overlay, but the test
    # framework owns each node's `pkgs`, so we wire packages by option instead.
    imports = [ ../modules/agent.nix ../modules/control-plane.nix ];

    # Control plane: server + local Postgres + NATS. No auth, no enrollment.
    # Use a TCP + trust Postgres so the server's sqlx URL is unambiguous (the
    # module's default unix-socket URL form isn't accepted by this sqlx version).
    services.ravn.controlPlane = {
      enable = true;
      bind = "127.0.0.1:8080";
      package = self.packages.${pkgs.stdenv.hostPlatform.system}.ravn-server;
      database.url = "postgres://ravn@127.0.0.1:5432/ravn";
    };
    services.postgresql.enableTCPIP = true;
    services.postgresql.authentication = lib.mkForce ''
      local all all trust
      host  all all 127.0.0.1/32 trust
      host  all all ::1/128      trust
    '';

    # Agent + actuator, remediation enabled, pointed at the local control plane.
    services.ravn.agent = {
      enable = true;
      package = self.packages.${pkgs.stdenv.hostPlatform.system}.ravn-agent;
      server.url = "nats://127.0.0.1:4222";
      # Base URL for command pull; with no bootstrap token the agent skips
      # enrollment but still uses this endpoint to pull commands.
      enrollment.endpoint = "http://127.0.0.1:8080";
      inference.enable = false;
      remediation = {
        enable = true;
        actuatorPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.ravn-actuator;
        commandSigningPublicKey = testPubKey;
        pollSecs = 2;
      };
    };

    # The server loads its templates and a fixed signing key from known paths.
    systemd.services.ravn-server.environment = {
      RAVN_TEMPLATES_DIR = "${self}/templates";
      RAVN_COMMAND_KEY = "/etc/ravn/command.key";
    };
    # The agent reads its pinned command public key from a fixed cred dir.
    systemd.services.ravnd.environment.RAVN_CRED_DIR = "/etc/ravn-creds";

    environment.etc."ravn/command.key".text = testPrivKey;
    environment.etc."ravn-creds/command_pubkey.b64".text = testPubKey;

    # A unit that can be healed: runs forever, but Restart=no so a SIGKILL leaves
    # it `failed`; a restart brings it back to `active`.
    systemd.services.dummy = {
      description = "Dummy healable unit";
      serviceConfig = {
        ExecStart = "${pkgs.coreutils}/bin/sleep infinity";
        Restart = "no";
      };
    };

    virtualisation.memorySize = 2048;
    virtualisation.diskSize = 4096;
  };

  testScript = ''
    import json

    machine.start()
    machine.wait_for_unit("postgresql.service")
    machine.wait_for_unit("nats.service")
    machine.wait_for_unit("ravn-server.service")
    machine.wait_for_unit("ravn-actuator.service")
    machine.wait_for_unit("ravnd.service")
    machine.wait_until_succeeds("curl -sf http://127.0.0.1:8080/health", timeout=90)

    # Drive the dummy unit into a failed state.
    machine.succeed("systemctl start dummy.service")
    machine.succeed("systemctl kill -s SIGKILL dummy.service")
    machine.wait_until_succeeds("systemctl is-failed dummy.service", timeout=30)

    # The failed-unit tap should yield a pending remediation proposal.
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

    # Approve: the server signs + enqueues, ravnd pulls + verifies, the actuator heals.
    machine.succeed(f"curl -sf -X POST http://127.0.0.1:8080/api/remediations/{rid}/approve")

    # The dummy unit returns to active...
    machine.wait_until_succeeds("systemctl is-active dummy.service", timeout=60)

    # ...and the record reports a successful result.
    machine.wait_until_succeeds(
        "curl -sf http://127.0.0.1:8080/api/remediations | grep -q '\"status\":\"succeeded\"'",
        timeout=60,
    )

    # The actuator must be the privileged one; ravnd stays unprivileged.
    machine.succeed("test \"$(systemctl show -p User --value ravnd.service)\" != root")
  '';
}
