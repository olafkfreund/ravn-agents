---
layout: page
title: Showcase
permalink: /showcase/
---

# Ravn in action

A working tour of the **Operator Console** — the live portal over the control
plane — driven by real Kubernetes and host signals, explained in plain language by
a small local model (CPU, or a GPU when you have one), with supervised self-healing.

![Ravn portal walkthrough]({{ '/assets/img/ravn-portal-demo.gif' | relative_url }})

*A walkthrough of the live portal: the fleet event feed, an event opened to its
AI explanation, the fleet inventory, the grouped topology view, and the
self-healing remediations.*

---

## A real-life scenario

> **3:41am.** The `payments` service in the **prod-eu** cluster starts getting
> killed under load.

Here's what happens, end to end — and crucially, **no language model is
anywhere near the decision that something is wrong**.

1. **Detection (deterministic).** The Ravn **controller** — a single read-only
   Deployment, no sidecars — is watching the cluster's Events API. The kubelet
   OOM-kills `payments/api-7d9f8`; an `OOMKilled` event fires. The controller
   maps the reason to a severity with a fixed table (`OOMKilled → Critical`) and
   emits a structured `KubeWorkload` event. This path is pure code: it would
   fire identically with the model switched off.

2. **Transport + persistence.** The event crosses to the control plane
   (authenticated — the agent presents its projected ServiceAccount token,
   validated against the cluster's OIDC JWKS), lands in Postgres, and is fanned
   out to every connected portal over a WebSocket. The alarm is already ringing.

3. **The last mile (the model).** *Off* the detection path, the control plane
   asks a shared inference endpoint to explain the event. The reply is attached
   to the stored event when it returns — and if the model is slow or down, the
   event simply keeps its deterministic title. Safe failure.

The operator opens the event and sees this:

![AI explanation of an OOMKilled pod]({{ '/assets/img/event-explanation.png' | relative_url }})

> **The pod's container exceeded its memory limit (512Mi) and was terminated by
> the OS to prevent further resource contention. This indicates a potential
> misconfiguration or uncontrolled memory usage in the container.**
>
> **Suggested check:** `kubectl top pod payments` · *via qwen3:1.7b*

Deterministic detection on the left; a plain-language explanation and a concrete
next command on the right — produced by the *same* small model (Qwen3 1.7B) the
host agents run locally under llama.cpp, so host and cluster signals read alike.

---

## The fleet, at a glance

Every host and cluster that reports in becomes a card with a live status and a
worst-severity rollup. Here three clusters — `prod-eu`, `prod-us`, `staging` —
sit alongside the local dev cluster.

![Fleet inventory]({{ '/assets/img/agents-fleet.png' | relative_url }})

The **topology** view groups the same fleet by a category of your choosing
(environment, region, team — whatever labels you attach), colour-coded by the most
severe live event per node, with a per-*kind* icon so different things read
differently. Here the one-box demo groups into a **☸ k3d cluster** (failing pods,
red) and a **❄ NixOS host** (healthy, green):

![Topology grouped into a k3d cluster and a NixOS host]({{ '/assets/img/topology.png' | relative_url }})

---

## It heals itself — with your approval

Detection closes the loop. When a deterministic fault matches a curated template,
the control plane proposes a fix; you approve it; the control plane signs an
Ed25519 command; the agent's privileged actuator runs exactly that typed action and
reports back — recorded at-most-once. The model is never in this path either.

![Remediations: approve a proposed fix; a clean history of healed actions]({{ '/assets/img/remediations.png' | relative_url }})

Kill a `flaky.service` on the demo host and watch it go
`proposed → approved → succeeded (active)` in a few seconds — or let an
auto-approve policy handle the low-risk ones. A default-deny gate, a circuit
breaker, and a kill switch keep it safe.

---

## How it's built

Ravn is a three-plane system that shares **one Rust type system** end to end, so
an event the agent emits is the exact shape the server persists and the portal
renders.

| Plane | What it is | Stack |
| --- | --- | --- |
| **Edge** | `ravnd` host agent (+ a privileged actuator) and the Kubernetes controller & node-agent | Rust, deterministic taps, kube-rs informers, an OpenAI-compatible model (llama.cpp / Ollama, CPU or GPU) for the local last mile |
| **Control plane** | `ravn-server` — ingest, persist, explain, propose + sign fixes, serve | Rust (axum), NATS, Postgres (partitioned time-series), OpenAI-compatible inference client, Ed25519 command signing, Prometheus metrics |
| **Portal** | the Operator Console | React + React Flow, typed against the server's OpenAPI |

**The guiding rule:** the LLM is *never* in the detection hot path. Deterministic
tooling decides whether something is wrong and fires the alarm; the model only
writes the explanation. A slow or wrong model degrades the wording, never the
alerting.

See the [Architecture]({{ '/architecture/' | relative_url }}) page for the full
design.

---

## What's shipped so far

- **Agent** — host detection taps (journald, failed units, config drift, auth,
  updates), local CPU-only inference with reactive + periodic-digest modes,
  resource caps, a NATS/WebSocket transport, and an offline buffer.
- **Control plane** — axum API + OpenAPI, NATS ingestion → Postgres, agent
  registry + category model, Prometheus metrics, and **auth**: agent
  credentials (mTLS enrollment + ServiceAccount-token ingest) and **portal user
  OIDC + RBAC**.
- **Kubernetes** — a workload **controller**, a node **DaemonSet** agent, OIDC /
  TokenReview ingest auth, control-plane **inference for K8s events**, deploy
  manifests + a Helm chart + OCI images, and a kind/k3d end-to-end test.
- **Supervised self-healing (Linux/NixOS hosts)** — a **PARR** loop
  (Prepare/Act/Reflect/Review): deterministic template matching (the LLM never
  decides), **Ed25519-signed** commands, a **privileged actuator** under privilege
  separation, an at-most-once ledger, a default-deny policy + circuit breaker,
  verify/rollback, and a retrospective knowledge base. *The in-cluster K8s executor
  has landed ([#146](https://github.com/olafkfreund/ravn-agents/issues/146)) and the
  audit trail is now durable in Postgres
  ([#143](https://github.com/olafkfreund/ravn-agents/issues/143)) — both pending final
  end-to-end verification (k3d / live Postgres) before release.*
- **One-box demo** — a NixOS host, a k3d cluster, and **GPU-accelerated** local
  explanations (AMD ROCm / NVIDIA / CPU), with a live `kill → propose → approve →
  heal` loop — `scripts/demo-up.sh`.
- **Observability & eval** — a model benchmark harness, prompt-regression
  fixtures, metrics, security hardening, and an end-to-end smoke test.
- **Portal** — live event feed, fleet inventory, a grouped topology view (with
  per-kind icons), and the **Remediations** approve/heal page.

Everything above is verified end to end — including a real k3d cluster whose
crashing pod's `BackOff` events flow through the controller, the control plane, and
into Postgres, and a host unit that is killed, proposed, approved, and healed live.

Browse the work, organised by epic with `good first issue` labels, on
[GitHub](https://github.com/olafkfreund/ravn-agents), and follow the
[devlog]({{ '/blog/' | relative_url }}) for decisions and struggles.
