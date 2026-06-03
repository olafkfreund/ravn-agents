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

## `services.ravn.controlPlane` (#36)

Declarative control plane. By default it provisions a local PostgreSQL (`ravn`
database, owned by the `ravn` role via socket peer auth) and a loopback NATS
with JetStream, and runs `ravn-server` as a hardened systemd service bound to
loopback.

```nix
{
  imports = [ ravn.nixosModules.controlPlane ];

  services.ravn.controlPlane = {
    enable = true;
    bind = "127.0.0.1:8080";        # front with a TLS reverse proxy for remote access
    # database.createLocally = true; # or set database.url for an external DB
    # nats.createLocally = true;     # or point nats.url at an external broker
    # oidc = { issuer = "https://idp.example.com"; clientId = "ravn"; };  # #26
  };
}
```

## Local inference (#15)

When `inference.enable` is true and a model is configured, the module runs
`llama-server` as a sandboxed, **loopback-only** systemd unit (`ravn-llama`) in
the resource-capped `ravn-inference` slice. The agent reaches it at
`http://<host>:<port>` (default `127.0.0.1:18181`).

Configure the model one of two ways:

```nix
services.ravn.agent.inference = {
  # (a) a GGUF already on the host:
  model.path = "/var/lib/ravn/models/qwen3-1.7b-q4_k_m.gguf";

  # (b) or pin one to fetch into the store:
  # model = {
  #   name = "qwen3-1.7b-q4_k_m";
  #   url = "https://…/Qwen3-1.7B-Q4_K_M.gguf";
  #   sha256 = "sha256-…";  # nix-prefetch-url the file to get this
  # };

  cpuQuota = "400%";   # sizes the ravn-inference slice (#18)
  memoryMax = "2G";
  contextSize = 4096;
};
```

**Swapping models:** change `model.path` (or `model.url`+`sha256` and
`model.name`) and rebuild; the unit restarts with the new model. If neither
`path` nor `url` is set, no llama-server unit is created and the agent simply
emits events without explanations (safe failure — detection never waits on it).

### Resource caps & bench hook (#18)

The `ravn-inference` slice caps CPU and memory (`cpuQuota`, `memoryMax`); the
`ravn-llama` process additionally runs at a lower scheduling priority so it
never competes with interactive work:

```nix
services.ravn.agent.inference = {
  threads = 0;                       # 0 = host physical cores (avoids SMT oversubscription)
  nice = 5;                          # process Nice priority
  ioSchedulingClass = "best-effort"; # none | realtime | best-effort | idle
  ioSchedulingPriority = 6;          # 0 (highest) .. 7 (lowest); best-effort/realtime only

  # Opt-in: sample llama-server throughput on a timer, appended as JSONL.
  bench = {
    enable = true;
    interval = "1h";                            # systemd time span
    outputFile = "/var/lib/ravn-bench/bench.jsonl";
  };
};
```

`threads = 0` resolves the physical-core count at runtime (the target's CPU is
unknown at evaluation time). The bench hook records
`{ts, model, tokens_per_sec}` per tick, feeding the eval epic (#8) with on-host
throughput trends — pair it with the `ravn-eval` harness for model comparison.

### Digest mode (#17)

Instead of explaining every event as it fires, batch a window of events into a
single "what changed and what looks off" digest. One inference call per window
bounds CPU and hides per-event latency; events still publish immediately (bare),
and per-event enrichment is skipped while digest mode is on.

```nix
services.ravn.agent.inference.digest = {
  enable = true;
  intervalSecs = 3600;     # emit a digest hourly
  maxEvents = 100;         # cap events summarized per digest
  minSeverity = "notice";  # scope: ignore info-level noise
};
```

## Agent enrollment (#19)

Agents authenticate to the control plane with a per-agent **mTLS client
certificate**, obtained at enrollment by exchanging a shared bootstrap token.
The control plane runs an internal CA and signs each agent's CSR, binding the
certificate's identity to the agent's `agent_id` (the CSR's claimed subject is
ignored, so an agent can't mint a cert for another identity). Re-enrollment is
idempotent — an agent that already holds credentials reuses them.

Control plane — enable the `/enroll` endpoint:

```nix
services.ravn.controlPlane.enrollment = {
  enable = true;
  caCertFile = "/var/lib/ravn/ca.crt";     # public; signs agent certs
  caKeyFile = "/run/secrets/ravn-ca.key";  # secret — delivered as a credential
  bootstrapTokenFile = "/run/secrets/ravn-bootstrap-token";
  certTtlDays = 90;
};
```

Agent — point it at the endpoint and give it the token:

```nix
services.ravn.agent = {
  server.url = "nats://control.example.com:4222";
  enrollment.endpoint = "https://control.example.com:8080";
  enrollment.bootstrapTokenFile = "/run/secrets/ravn-bootstrap-token";
};
```

The CA key and bootstrap token are passed via systemd credentials and read at
runtime — never copied into the world-readable Nix store. Issued credentials
land in the agent's `StateDirectory` (`agent.key` `0600`, `agent.crt`,
`ca.crt`). Generate a CA with, e.g.:

```sh
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out ca.key
openssl req -x509 -new -key ca.key -days 3650 -subj '/CN=Ravn CA' -out ca.crt
```

> The transport's mTLS *handshake* enforcement (NATS/HTTP client-cert auth) is
> tracked in #26; #19 establishes and persists the credential.

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
