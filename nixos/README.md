# Ravn NixOS modules

Declarative deployment of Ravn on NixOS. The modules are exported from the
flake as `nixosModules`.

## `services.ravn.agent` (#35)

Runs the `ravnd` detection agent as a hardened systemd service (DynamicUser,
`ProtectSystem=strict`, syscall-filtered, journal-read access for the taps).
The bootstrap token is delivered via systemd credentials, never the Nix store.

```nix
{
  inputs.ravn.url = "github:olafkfreund/ravn-agents";

  # in a host's configuration:
  imports = [ ravn.nixosModules.agent ];

  services.ravn.agent = {
    enable = true;
    server.url = "nats://control.example.com:4222";
    enrollment.bootstrapTokenFile = "/run/secrets/ravn-bootstrap-token";
    detection.configDrift.paths = [ "/etc/nixos" "/etc/ssh/sshd_config" ];
    inference = {
      cpuQuota = "400%";
      memoryMax = "2G";
    };
  };
}
```

> The control-plane module (`services.ravn.controlPlane`, #36) and the
> `llama-server` inference unit (#15) land in later issues. This module wires
> the agent and sizes the inference slice; it does not yet start llama-server.

## Trying it out

`nixosConfigurations.demo-agent` is a minimal example machine (container
profile, cheap to evaluate and build):

```sh
nix build .#nixosConfigurations.demo-agent.config.system.build.toplevel
```

## Growing into a fleet

A "group of servers" is just more nodes consuming the same module. The
idiomatic test vehicle is a multi-node NixOS VM test
(`pkgs.testers.runNixOSTest`) with one control-plane node and N agent nodes —
real systemd/journald/D-Bus, runs under `nix flake check`. That harness arrives
with the end-to-end smoke test (#41) once the M0 transport (#22) and control
plane ingestion (#24) exist; until then there is no end-to-end thread to assert.
