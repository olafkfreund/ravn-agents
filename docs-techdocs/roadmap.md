---
layout: page
title: Roadmap
permalink: /roadmap/
---

This is the public plan. Timings are **indicative for a part-time, open-source pace**
and will move as people get involved — treat them as sequencing, not promises. Each
milestone maps to one or more [epics](https://github.com/olafkfreund/ravn-agents/labels/epic).

| Milestone | Theme | Indicative window |
|-----------|-------|-------------------|
| M0 | Walking skeleton (end-to-end thread) | Weeks 1–2 |
| M1 | Detection + local inference | Weeks 3–6 |
| M2 | Portal: inventory + live feed | Weeks 6–9 |
| M3 | Topology view + categories | Weeks 9–11 |
| M4 | Packaging (NixOS, OCI) + auth hardening | Weeks 11–14 |
| M5 | Alert routing + commands + eval harness | Weeks 14–18 |

## M0 — Walking skeleton

**What.** One real thread through all three planes: an agent emits a single event,
the control plane persists it, the portal lists it.

**How.** Stand up the Cargo workspace (`ravn-core`, `ravn-agent`, `ravn-server`),
define a minimal `Event` type, wire the agent to the server over plain WebSocket
(NATS comes in M1), persist to Postgres, scaffold the React portal with a single
inventory table.

**When.** First, ~1–2 weeks. Proof the shape works and the base everyone builds on.

## M1 — Detection + local inference

**What.** Real detection taps and real LLM explanations.

**How.** journald, failed-unit (D-Bus), config-drift (inotify+hash), auth/SSH and
NixOS-generation taps, normalised into `ravn-core` events. `llama-server` as a
sandboxed systemd unit with a pinned Qwen3 1.7B model; reactive prompt template;
SQLite offline buffer. Swap WebSocket for NATS.

**When.** ~3–4 weeks after M0. Delivers the core product promise.

## M2 — Portal: inventory + live feed

**What.** A usable operator UI.

**How.** Inventory (status, health, filter/search), a live message feed over
WebSocket with severity coding and a detail drawer (raw event + explanation +
suggested check), and per-agent detail/timeline. Tailwind + shadcn/ui.

**When.** Overlaps the back half of M1; ~3 weeks.

## M3 — Topology view + categories

**What.** The category-grouped diagram showcase.

**How.** Category model + API (user-defined tags, chosen grouping dimension), a React
Flow canvas with agents as nodes grouped into category containers, elk/dagre layout,
colour-coding, filter/search, and a category-management UI.

**When.** ~2 weeks after M2.

## M4 — Packaging + auth hardening

**What.** Make it deployable and safe by default.

**How.** Nix flake + `services.ravn.agent` and `services.ravn.controlPlane` modules;
OCI images + docker-compose. Harden enrollment (bootstrap tokens → per-agent
creds/mTLS), add user OIDC + RBAC, sandbox inference properly.

**When.** ~3 weeks. The point where others can realistically run a fleet.

## M5 — Alert routing + commands + eval

**What.** Production polish.

**How.** External alert sinks (ntfy, webhook, email, Slack) with routing rules;
push-commands to agents over NATS request/reply; a model-eval harness (tokens/sec +
quality on target CPUs) and prompt-regression tests against golden fixtures.

**When.** ~4 weeks. Beyond this: more detection taps, more models, dashboards.
