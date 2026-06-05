---
layout: page
title: Architecture
permalink: /architecture/
---

# Ravn — Architecture

Ravn is a three-plane system: **edge agents**, a **control plane**, and a **portal**.

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
- **Local inference:** loopback HTTP to a pinned `llama-server` (llama.cpp).
  Default model Qwen3 1.7B Q4_K_M (~1.3 GB). Two modes: *reactive* (explain a flagged
  event) and *periodic digest* (batched summary; hides latency, batches CPU).
- **Buffering:** local SQLite queue for offline operation, dedupe and rate-limiting.
- **Resource control:** `llama-server` runs in its own systemd slice with
  `CPUQuota`, `MemoryMax`, `Nice`/`IOSchedulingPriority`. Idle cost is near-zero;
  spikes are short and bounded.

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
  **Users**.
- API: REST + WebSocket/SSE for the live feed; OpenAPI spec.
- Auth: agents via NATS credentials (nkey/JWT) or mTLS, enrolled with bootstrap
  tokens; users via OIDC (Authelia/Keycloak) with viewer/admin roles.

## Portal (React + TypeScript)

Vite + Tailwind + shadcn/ui + TanStack Query, WebSocket for live updates.

- **Fleet inventory** — agents, status, health, filter/search by category.
- **Live message feed** — severity-coded, detail drawer showing raw event + LLM
  explanation + suggested check.
- **Agent detail** — timeline, recent events, config drift, inference stats.
- **Topology showcase** — React Flow (`@xyflow/react`), nodes = agents, user-defined
  categories as group containers, elk/dagre layout, colour-coded, filterable.
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
