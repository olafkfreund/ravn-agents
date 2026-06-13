---
layout: page
title: Roadmap
permalink: /roadmap/
---

This is the public plan. Timings are **indicative for a part-time, open-source pace**
and will move as people get involved — treat them as sequencing, not promises. Each
milestone maps to one or more [epics](https://github.com/olafkfreund/ravn-agents/labels/epic).

| Milestone | Theme | Status |
|-----------|-------|--------|
| M0 | Walking skeleton (end-to-end thread) | ✅ Shipped |
| M1 | Detection + local inference | ✅ Shipped |
| M2 | Portal: inventory + live feed | ✅ Shipped |
| M3 | Topology view + categories | ✅ Shipped |
| M4 | Packaging (NixOS, OCI) + auth hardening | ✅ Shipped |
| M5 | Kubernetes plane (controller + node-agent) | ✅ Shipped |
| M6 | Supervised self-healing (PARR) — **hosts** | ✅ Shipped (P1–P4); K8s execution deferred |
| M7 | Alert routing + dashboards | 🚧 Next |

## ✅ M0–M3 — Skeleton, detection, portal, topology

The foundation is in place and verified end to end: the Cargo workspace
(`ravn-core`, `ravn-agent`, `ravn-server`), deterministic detection taps (journald,
failed-unit over D-Bus, config drift, auth/SSH, NixOS generations), local LLM
explanations, NATS transport with a SQLite offline buffer, and the React portal —
fleet inventory, a live severity-coded event feed with a detail drawer, and the
**topology view** that groups the fleet by a label of your choosing (now with a
per-*kind* icon, so a cluster and a host read differently at a glance).

## ✅ M4 — Packaging + auth hardening

Deployable and safe by default: a Nix flake with `services.ravn.agent` and
`services.ravn.controlPlane` modules, OCI images + docker-compose, hardened
enrollment (bootstrap tokens → per-agent mTLS), portal user **OIDC + RBAC**, and
systemd-sandboxed inference.

## ✅ M5 — Kubernetes plane

Ravn watches Kubernetes the same way it watches hosts, detection-only: a read-only
**controller** Deployment over the Events API and a node **DaemonSet** agent, with
OIDC / TokenReview ingest auth, control-plane **inference for K8s events**, deploy
manifests + a Helm chart, and a kind/k3d end-to-end test. Cluster signals and host
signals share one `ravn-core` type system and read alike in the portal.

## ◑ M6 — Supervised self-healing (PARR), on hosts

The big one. Detection closes the loop — under human control by default. Shipped
for **Linux/NixOS hosts**; the Kubernetes execution surface is deferred (see below).
The loop runs through five phases; here is the honest per-phase status.

- ✅ **P1 — Prepare / Act / Reflect / Review (PARR), manual-approval-only.** A
  detected fault is matched against a curated, typed **remediation template**;
  the control plane *proposes* a fix. On approval it issues an **Ed25519-signed**
  command; the agent pulls it and a **privileged actuator** (privsep) carries it
  out. Matching is **deterministic** — the LLM is never in the propose/act path
  (it explains, it never decides; *no LLM-generated commands, ever*).
- ✅ **P2 — Autonomy.** Default-deny policy engine + risk tiers, circuit breaker,
  and a kill switch. An optional policy can auto-approve low-risk actions.
- ✅ **P3 — Safety net.** Verify and rollback (including NixOS generation rollback)
  and an **at-most-once** idempotency ledger.
- ✅ **P4 — Knowledge base.** Retrospectives accumulate into per-environment
  markdown with deterministic recall, so the fleet gets better at explaining
  itself over time.
- 🚧 **P5 — Breadth & K8s execution.** The **Kubernetes execution surface** is
  **deferred**: `templates/k8s-pod-restart.toml` and `k8s-pod-log-restart.toml`
  exist and the controller detects + the server proposes, but there is **no
  in-cluster executor yet** — host remediation only runs today. Tracked by
  [#146](https://github.com/olafkfreund/ravn-agents/issues/146). Approval via
  alert sinks (Slack/ntfy actionable buttons) is also part of this phase.

> **Durability caveat.** The remediation audit trail is currently in-memory and is
> lost on a control-plane restart; moving it to Postgres is
> [#143](https://github.com/olafkfreund/ravn-agents/issues/143).

The host loop runs on one machine via the [demo](https://github.com/olafkfreund/ravn-agents/blob/main/demo/README.md):
a NixOS host, a k3d cluster (detection-only), **GPU-accelerated** local explanations
(AMD ROCm / NVIDIA / CPU), and a live `kill → propose → approve → heal` loop on the host.

## 🚧 M7 — Alert routing + dashboards

**What.** Production polish.

**How.** External alert sinks (ntfy, webhook, email, Slack) with routing rules by
severity/category — **not yet wired**: there is no alert-routing backend endpoint
today, and the portal does not yet expose routing configuration. Plus richer portal
dashboards. Beyond this: more detection taps, more remediation templates, more models.

The model-eval harness (`ravn-eval`) is a separate track: a runnable benchmark today
(prompt/generation throughput, latency, memory, a deterministic quality score) — its
remaining work is the **fixture corpus and a published comparison page**, tracked by
[#157](https://github.com/olafkfreund/ravn-agents/issues/157).
