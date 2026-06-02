---
layout: home
title: Ravn
list_title: Devlog
---

**Small local-LLM agents that watch your Linux servers and report back in plain language.**

Ravn is a fleet of lightweight agents — one per host — that watch logs, services,
network, access, config drift and updates. Detection is deterministic and fast; a
small CPU-only language model runs the *last mile*, turning a flagged event into a
clear explanation and a suggested next check. A central control plane collects
everything, and a modern portal gives you fleet inventory, a live message feed, and
a category-grouped topology view.

Named for the raven — Odin's scouts that fly out across the world and return to tell
him what they saw.

### The idea in one line

The LLM is **never** in the detection hot path. Deterministic tooling decides whether
something is wrong and fires the alarm; the model only writes the explanation. A slow
or wrong model degrades the wording, never the alerting.

### What / How / When

- **What** — three planes: edge agents (`ravnd`), a control plane (`ravn-server`),
  and a web portal.
- **How** — Rust agent + server sharing one type system, NATS transport, Postgres,
  a React portal with a React Flow topology view, CPU-only inference via llama.cpp.
- **When** — milestones M0 (walking skeleton) → M5 (alert routing + eval harness).
  See the [Roadmap]({{ '/roadmap/' | relative_url }}).

### Get started

- Read the [Architecture]({{ '/architecture/' | relative_url }}) and
  [Roadmap]({{ '/roadmap/' | relative_url }}).
- Come [Get Involved]({{ '/get-involved/' | relative_url }}) — issues are organised
  by epic with `good first issue` labels.
- Follow the devlog below for progress, decisions, and struggles.
