# Ravn

**Self-hosted self-healing for Linux fleets — deterministic detection, signed and
auditable remediation, and AI that explains but never decides.**

Ravn watches hosts and clusters — logs, services, network, access, config drift,
updates — with fast, deterministic detection: rules you can read, not a model you
have to trust. Faults are matched against pre-authored remediation templates; a
human (or signed policy) approves; a privilege-separated actuator executes typed,
whitelisted capabilities with Ed25519-signed commands and a full audit trail. A
small local language model runs the *last mile* only, turning flagged events into
plain-language explanations. Runs on standalone Linux hosts, Kubernetes, and
fully air-gapped networks — inference is local and CPU-only.

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
