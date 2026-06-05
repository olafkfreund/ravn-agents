# Security

Ravn is built to be safe-by-default. This is the current security posture and
the known gaps (tracked as issues).

## Reporting

Please report vulnerabilities privately via a GitHub security advisory on
[ravn-agents](https://github.com/olafkfreund/ravn-agents/security/advisories),
not a public issue.

## Posture

### systemd sandboxing (NixOS modules)

Both `services.ravn.agent` and `services.ravn.controlPlane` run under a strict
sandbox:

- `DynamicUser` (agent) / a dedicated least-privilege `ravn` user (control
  plane); `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`,
  `PrivateTmp`, `PrivateDevices`, `Protect{Kernel*,ControlGroups,Proc,Clock,Hostname}`.
- `RestrictSUIDSGID`, `RestrictRealtime`, `RestrictNamespaces`,
  `LockPersonality`, `SystemCallFilter=@system-service` (minus `@privileged`/`@resources`),
  `RestrictAddressFamilies` limited to UNIX/INET.
- `MemoryDenyWriteExecute` on `ravnd` and the control plane; relaxed **only** for
  the `llama-server` inference unit (ggml needs executable memory), which is
  additionally confined to its own resource-capped slice and bound loopback-only
  (`IPAddressDeny=any` + `IPAddressAllow=localhost`).
- The agent gets just `systemd-journal` supplementary group access for the
  journald/auth taps — nothing more.

### Secret handling

- The enrollment bootstrap token is delivered via systemd `LoadCredential`,
  read at runtime — **never** written to the world-readable Nix store or the
  generated TOML config.
- Generated config (`pkgs.formats.toml`) is for non-secret settings only.
- State (the SQLite offline buffer) lives under a `0700` `StateDirectory`.

### Least privilege & network

- The Kubernetes controller (planned, #53) uses a **read-only** ClusterRole.
- Control-plane and inference bind **loopback** by default; expose via a TLS
  reverse proxy.
- Inference is off the detection hot path: a slow or compromised model degrades
  wording, never the alerting.

## Known gaps (tracked)

- **Transport/API auth (#26):** ingestion (NATS) and the read API are currently
  unauthenticated. Agent credentials (NATS nkey/JWT or mTLS) and portal OIDC/RBAC
  are pending. Do not expose the control plane to untrusted networks yet.
- The dev portal uses a permissive CORS layer; tighten before production.
- The K8s `ServiceAccount`-token auth design is specified (#57) but not built.

## CI

`nix flake check` (build + clippy `--deny warnings` + tests + NixOS toplevels +
OCI image evals) and an end-to-end smoke test run on every push and pull request.
