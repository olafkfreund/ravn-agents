# Remediation template library

Copy-paste starting points for common remediation scenarios. **These are
examples — they are NOT auto-loaded.** Copy the ones you want into your control
plane's templates directory (the dir passed to `TemplateRegistry::load_dir`,
e.g. `RAVN_TEMPLATES_DIR` / the `templates/` mount) and restart the server. The
server validates every template at startup; a bad condition path or an
undeclared `{{placeholder}}` fails the boot loudly (issue #151), so a typo can
never silently no-op.

## How a template works

A template binds a **detected fault** to a sequence of **typed capabilities**:

```
match (source + field conditions)  ──▶  proposal
  parameters   (pull values from the event)
  preconditions (read-only checks that must ALL hold before acting)
  steps         (the mutating actions, in order)
  verify        (read-only check + expected value, within timeout_s)
  rollback      (none | nix_generation)
```

Everything the actuator runs is **Ed25519-signed, policy-gated, and audited**.
There is no arbitrary-shell capability by design.

## The capability set (the "approved list")

| Capability | Kind | Fields | Notes |
|---|---|---|---|
| `unit_state` | read | `unit` | systemd `is-active` state — use in preconditions/verify |
| `reset_failed` | mutate | `unit` | `systemctl reset-failed` |
| `restart_unit` | mutate | `unit` | `systemctl restart` |
| `nix_rollback` | mutate | — | roll the host to its previous NixOS generation |
| `pod_state` | read | `namespace`, `pod_prefix` | K8s pod phase/reason — preconditions/verify |
| `delete_pod` | mutate | `namespace`, `pod` | delete a controller-owned pod so it's recreated |
| `restart_deployment` | mutate | `namespace`, `deployment` | `kubectl rollout restart` equivalent |

## Match sources & the fields you can condition on (equality only)

| `source` | useful `payload.*` paths |
|---|---|
| `failed_unit` | `payload.unit`, `payload.result` |
| `journald` | `payload.unit`, `payload.message`, `severity` |
| `config_drift` | `payload.path`, `payload.new_hash` |
| `auth` | `payload.action`, `payload.user`, `payload.remote_addr` |
| `kube_workload` | `payload.reason`, `payload.namespace`, `payload.name`, `payload.object_kind` |
| `kube_node` | `payload.condition`, `payload.node` |

Conditions are **exact-equality** on a field's value (e.g. `payload.reason = "OOMKilled"`).

## Risk tiers control automation

- `safe` — idempotent, no data loss. **Eligible for auto** where policy allows.
- `guarded` — service-affecting but reversible. Use to **force manual approval**
  per-service even where Safe-tier is auto-approved.
- `dangerous` — wide blast radius. **Never auto-executes** — always manual.

## What you CANNOT express today (be honest with your teams)

- **Substring/regex log matching** — conditions are exact-equality, so you can't
  match "message contains OOM". (K8s OOM works because the controller sets a
  typed `payload.reason = "OOMKilled"`.)
- **Auth remediation** — there's no "block IP" / "lock user" capability, so auth
  events are detect-only.
- **Node drain / cordon / scale** — no node- or replica-level mutating capability.
- **Arbitrary scripts/commands** — by design. Adding a new action means adding a
  *typed* capability (Rust change), so it stays signed and auditable.

## Files here

| File | Source | Tier | Action |
|---|---|---|---|
| `nginx-restart.toml` | failed_unit (nginx) | safe | reset-failed + restart, verify active |
| `postgresql-restart.toml` | failed_unit (postgres) | guarded | restart, long verify, **manual** |
| `app-restart-dependency-gated.toml` | failed_unit (my-app) | guarded | restart only if a dependency is healthy, **manual** |
| `k8s-oomkilled-pod-restart.toml` | kube_workload (OOMKilled) | safe | delete pod, verify Running |
| `critical-config-drift-rollback.toml` | config_drift | dangerous | nix_rollback, **manual** |

Each is a pattern — change the unit/namespace/path and tier to fit your services.
