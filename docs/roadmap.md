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
| M6 | Supervised self-healing (PARR) | ✅ Shipped |
| M7 | Alert routing + eval + dashboards | 🚧 Next |

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

## ✅ M6 — Supervised self-healing (PARR)

The big one. Detection now closes the loop — under human control by default:

- **Prepare / Act / Reflect / Review (PARR).** A detected fault is matched against a
  curated, typed **remediation template**; the control plane *proposes* a fix.
- **Approve → sign → act.** On approval the control plane issues an **Ed25519-signed**
  command; the agent pulls it and a **privileged actuator** (privsep) carries it out.
- **Safe by construction.** Default-deny policy engine + circuit breaker, an
  **at-most-once** idempotency ledger, and verify/rollback. An optional policy can
  auto-approve low-risk actions; a kill switch disables execution entirely.
- **Knowledge base.** Retrospectives accumulate into per-environment markdown with
  deterministic recall, so the fleet gets better at explaining itself over time.

It runs on one machine via the [demo](https://github.com/olafkfreund/ravn-agents/blob/main/demo/README.md):
a NixOS host, a k3d cluster, **GPU-accelerated** local explanations (AMD ROCm /
NVIDIA / CPU), and a live `kill → propose → approve → heal` loop.

## 🚧 M7 — Alert routing + eval + dashboards

**What.** Production polish.

**How.** External alert sinks (ntfy, webhook, email, Slack) with routing rules by
severity/category; a growing model-eval harness (tokens/sec + quality on target
CPUs *and* GPUs) and prompt-regression tests against golden fixtures; richer portal
dashboards. Beyond this: more detection taps, more remediation templates, more
models.
