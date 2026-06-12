---
layout: home
title: Ravn
list_title: Devlog
---

**Self-hosted self-healing for Linux fleets — deterministic detection, signed and
auditable remediation, and AI that explains but never decides.**

Ravn watches your hosts and clusters — logs, services, network, access, config
drift, updates — with fast, deterministic detection: rules you can read, not a
statistical model you have to trust. When something breaks, Ravn matches it
against pre-authored remediation templates; a human (or signed policy) approves;
a tiny privileged actuator executes typed, whitelisted capabilities — every
command Ed25519-signed, verified, and audited. A small local language model runs
the *last mile* only: turning a flagged event into a clear explanation and a
suggested next check.

It runs on **standalone Linux hosts**, on **Kubernetes**, and on **fully
air-gapped networks** — inference is local and CPU-only, so nothing has to leave
your infrastructure.

Named for the raven — Odin's scouts that fly out across the world and return to tell
him what they saw.

### The idea in one line

The LLM is **never** in the detection or action hot path. Deterministic tooling
decides whether something is wrong; signed templates and default-deny policy decide
what runs; the model only writes the explanation. A slow or wrong model degrades
the wording — never the alerting, never the fix.

### What / How / When

- **What** — three planes: edge agents (`ravnd`) with a privilege-separated
  actuator, a control plane (`ravn-server`) with policy and approvals, and a web
  portal with a remediation audit trail.
- **How** — Rust agent + server sharing one type system, NATS transport, Postgres,
  a React portal with a React Flow topology view, local CPU inference via
  llama.cpp (a shared inference endpoint is supported too).
- **When** — see the [Roadmap]({{ '/roadmap/' | relative_url }}), which says
  honestly what runs today and what is still in flight.

### Get started

- **See it in action** — the [Showcase]({{ '/showcase/' | relative_url }}) walks
  through the live portal and a real Kubernetes OOMKill, explained by the model.
- Read the [Architecture]({{ '/architecture/' | relative_url }}) and
  [Roadmap]({{ '/roadmap/' | relative_url }}).
- Come [Get Involved]({{ '/get-involved/' | relative_url }}) — issues are organised
  by epic with `good first issue` labels.
- Follow the devlog below for progress, decisions, and struggles.
