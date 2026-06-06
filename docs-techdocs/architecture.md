---
layout: page
title: Architecture
permalink: /architecture/
---

# Ravn — Architecture

Ravn is a three-plane system — **edge agents**, a **control plane**, and a
**portal** — that watches Linux/NixOS **hosts** and **Kubernetes** clusters,
explains what it finds in plain language, and (with your approval) heals it.

## Guiding principle

The LLM is never in the detection hot path. Deterministic tooling decides whether
something is wrong and fires the alarm; the small model only turns a pre-filtered
event into a human-readable explanation and a suggested check. A slow or wrong model
degrades the *wording*, never the *alerting* — a safe failure mode.

## Edge — `ravnd` (Rust)

- **Detection taps (deterministic):**
  - journald reader (`systemd` crate)
  - failed-unit state over D-Bus (`zbus`)
  - config drift via inotify (`notify`) + content hashing/diff
  - SSH/auth + audit events from the journal
  - update detection (NixOS generation/derivation changes; apt/dnf elsewhere)
- **Normalization:** every signal becomes a typed `Event` (defined in `ravn-core`),
  with severity, source, host and timestamp.
- **Local inference:** an OpenAI-compatible endpoint (llama.cpp's `llama-server`
  or Ollama). Default model Qwen3 1.7B (~1.4 GB) — comfortable on CPU, and the
  demo runs it on a GPU (AMD ROCm / NVIDIA) for near-instant explanations. Two
  modes: *reactive* (explain a flagged event inline, before publish) and
  *periodic digest* (batched summary; hides latency, batches compute).
- **Remediation client:** when enabled, pulls **signed** commands from the control
  plane and relays them to a privileged actuator (see *Supervised self-healing*).
- **Buffering:** local SQLite queue for offline operation, dedupe and rate-limiting.
- **Resource control:** `llama-server` runs in its own systemd slice with
  `CPUQuota`, `MemoryMax`, `Nice`/`IOSchedulingPriority`. Idle cost is near-zero;
  spikes are short and bounded.

## Edge — Kubernetes (`ravn-k8s`)

Ravn watches clusters the same detection-only way it watches hosts — no sidecars,
no mutation:

- **Controller** — one read-only Deployment watching the cluster Events API,
  mapping reasons (`CrashLoopBackOff`, `OOMKilled`, `ImagePullBackOff`, …) to typed
  `KubeWorkload`/`KubeNode` events with a fixed severity table.
- **Node-agent** — a DaemonSet, one pod per node, watching node status conditions.
- **Least privilege** — read-only RBAC (get/list/watch). Ingest is authenticated
  with a projected ServiceAccount token validated against the cluster's OIDC JWKS,
  with a Kubernetes TokenReview fallback.

The same `ravn-core` `Event` type carries host and cluster signals, so they read
identically in the portal and group together in the topology.

## Transport — NATS

Agents connect **outbound only** (firewall-friendly) and publish to subjects like
`ravn.<host>.events`. NATS request/reply leaves a clean path for later push-commands
(re-run digest, mute). Alternatives considered: gRPC streaming (no broker, more
boilerplate) and plain HTTPS+WebSocket (simplest, used as the M0 fallback).

## Control plane — `ravn-server` (Rust + Axum)

- Subscribes to NATS, validates, persists to **PostgreSQL** (SQLx); messages as a
  partitioned time-series table (Timescale optional later).
- Domain: **Agents** (registry, last-seen, health), **Events/Messages**,
  **Categories** (user-defined tags + a chosen grouping dimension for the diagram),
  **Remediations** (proposals, decisions, signed commands, results), **Users**.
- **K8s-event explanations:** asynchronously, off the ingest path, the control
  plane asks the inference endpoint to explain bare cluster events and attaches the
  result when it returns. Host events are explained on the agent; cluster events
  here — same model, consistent wording.
- API: REST + WebSocket/SSE for the live feed; OpenAPI spec.
- Auth: agents via NATS credentials (nkey/JWT) or mTLS, enrolled with bootstrap
  tokens; cluster ingest via OIDC/TokenReview; users via OIDC (Authelia/Keycloak)
  with viewer/admin roles.

## Supervised self-healing (PARR)

Detection closes the loop, under human control by default. The cycle is
**Prepare → Act → Reflect → Review**:

- **Prepare.** A detected fault is matched against a curated, typed **remediation
  template** (TOML); the control plane emits a *proposal* with a risk tier.
- **Act.** On approval the control plane mints an **Ed25519-signed** `CommandEnvelope`.
  The agent pulls it, verifies the signature against a pinned key, and hands it to a
  **privileged actuator** running under privilege separation. The actuator executes
  exactly the typed capability (e.g. restart a unit) — never arbitrary shell.
- **Reflect.** The actuator verifies the new state and reports a result; the agent
  records it in an **at-most-once** idempotency ledger so a command never runs twice.
- **Review.** A **default-deny** policy engine plus a circuit breaker gate what may
  run; an optional policy can auto-approve low-risk actions, and a kill switch
  disables execution entirely. Retrospectives feed a per-environment **knowledge
  base** (markdown with deterministic recall).

The detection path is unchanged: the model still never decides whether something is
wrong, and a remediation only ever runs because a deterministic fault matched a
template *and* the gate allowed it.

## Portal (React + TypeScript)

Vite + Tailwind + shadcn/ui + TanStack Query, WebSocket for live updates.

- **Fleet inventory** — agents, status, health, filter/search by category.
- **Live message feed** — severity-coded, detail drawer showing raw event + LLM
  explanation + suggested check.
- **Agent detail** — timeline, recent events, config drift, inference stats.
- **Topology showcase** — React Flow (`@xyflow/react`), nodes = agents grouped by a
  chosen label into containers, with a per-*kind* icon (☸ cluster, ❄ NixOS, 🖥 host),
  colour-coded by worst live severity, filterable.
- **Remediations** — pending proposals to approve (which signs + dispatches the
  command) and a history of past actions and their results.
- **Category management** and **settings** (enrollment tokens, users, alert routing).

Full-Rust alternative (Leptos/Dioxus) is viable but React Flow's maturity wins for
the topology view.

## Output / alerting

Control plane fans messages to the in-app feed plus external sinks (ntfy, webhook,
email, Slack), with routing rules by severity/category.

## Cross-cutting

- Cargo workspace; shared `ravn-core` types across agent and server.
- `tracing` + OpenTelemetry + Prometheus metrics (Ravn can watch its own control plane).
- Nix flake with `services.ravn.agent` and `services.ravn.controlPlane` modules; OCI
  images + docker-compose for non-Nix hosts.
- Model-eval harness (tokens/sec + quality on target CPUs) and prompt-regression tests
  against golden log fixtures.
