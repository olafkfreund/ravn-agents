# Ravn

**Small local-LLM agents that watch your Linux/NixOS hosts and Kubernetes, report
in plain language, and — with supervision — heal themselves.**

Ravn is a fleet of lightweight agents (one per host) that watch logs, services,
network, access, config drift and updates. Detection is deterministic and fast; a
small CPU-only language model runs the *last mile*, turning a flagged event into a
clear, human-readable explanation and a suggested next check. A central control
plane collects everything, a portal gives you inventory, a live feed and a
topology view, and a supervised remediation loop can fix recurring faults.

!!! info "This is the TechDocs view"
    This site is generated for Backstage from the repository. The public
    marketing/blog site lives separately at
    [olafkfreund.github.io/ravn-agents](https://olafkfreund.github.io/ravn-agents/).
    Both are driven from the same repo — see [Backstage integration](backstage.md).

## Start here

- **[Architecture](architecture.md)** — the three planes (edge agents, control plane, portal) and the guiding "LLM is never in the detection hot path" principle.
- **[Roadmap](roadmap.md)** — milestones M0–M5 and where things stand.
- **[Design specs](design/index.md)** — the in-repo design documents (Kubernetes, self-healing remediation, and implementation plans).
- **[Get involved](get-involved.md)** / **[Contributing](contributing.md)** — dev setup and how to help.
- **[Security](security.md)** — reporting vulnerabilities and the project's security posture.

## The system at a glance

| Component | What it does |
|-----------|--------------|
| `ravnd` (ravn-agent) | Edge agent: deterministic detection taps + local inference; relays signed remediation commands. |
| `ravn-server` | Control plane: ingest, persist, serve the portal API, sign remediation commands. |
| `ravn-actuator` | The only privileged component: runs typed, whitelisted remediation capabilities. |
| `ravn-controller` | Kubernetes detection (controller + node DaemonSet). |
| `ravn-portal` | Web UI: inventory, live feed, topology. |
| `ravn-core` / `ravn-crypto` | Shared schema and command-signing crypto. |

See the [control-plane API](https://github.com/olafkfreund/ravn-agents/blob/main/portal/openapi.json)
and the full catalog model in
[`catalog-info.yaml`](https://github.com/olafkfreund/ravn-agents/blob/main/catalog-info.yaml).
