# Ravn

**Self-hosted self-healing for Linux fleets — deterministic detection, signed and
auditable remediation, and AI that explains but never decides.**

Ravn watches your hosts and clusters — logs, services, network, access, config
drift, updates — with fast, deterministic detection: rules you can read, not a
statistical model you have to trust. When something breaks, Ravn matches the fault
against pre-authored, reviewed remediation templates; a human (or a signed policy)
approves; a tiny privileged actuator executes typed, whitelisted capabilities. Every
command is Ed25519-signed, precondition-checked, verified after execution, and
written to an audit trail. A small local language model runs the *last mile* only:
turning a flagged event into a plain-language explanation and a suggested next
check. It never decides that something is wrong, and it never decides what to run.

Runs on **standalone Linux hosts**, on **Kubernetes**, and on **fully air-gapped
networks** — the model is local (CPU is enough), so nothing ever has to leave your
infrastructure.

Named for the raven — Odin's scouts that fly out across the world and return to
tell him what they saw.

> Status: early development, building in the open. Some pieces are ahead of
> others — the [Roadmap](docs/roadmap.md) says honestly what runs today and what
> is still in flight. Come say hello in
> [Discussions](https://github.com/olafkfreund/ravn-agents/discussions).

## Why Ravn

- **The LLM is never in the detection or action path.** Deterministic tooling
  decides *whether* something is wrong; signed templates and policy decide *what
  runs*. The model only writes the explanation. A slow or wrong model degrades
  the wording — never the alerting, never the fix.
- **Auditable by design.** Default-deny policy with risk tiers, Ed25519-signed
  command envelopes, a privilege-separated actuator that re-verifies every
  signature, circuit breakers, a fleet kill switch, and an audit record for every
  proposal, approval, and outcome. *(The audit store is now durable in Postgres —
  append-only, restart-safe — per [#143](https://github.com/olafkfreund/ravn-agents/issues/143).)*
- **Self-hosted everywhere.** Single static binaries, first-class NixOS modules,
  OCI images, Kubernetes manifests. No SaaS dependency, no vendor lock-in, no
  data leaving your network — air-gapped deployments are a supported target, not
  an afterthought, because inference runs locally on CPU.

## Architecture at a glance

Three planes:

- **Edge** — `ravnd`, the agent on each host (plus a read-only controller per
  Kubernetes cluster): detection taps, local inference, and — on hosts — a small
  privileged actuator for approved fixes. The in-cluster K8s executor has now
  landed ([#146](https://github.com/olafkfreund/ravn-agents/issues/146)) — signed,
  re-verified in-cluster, least-privilege RBAC — pending k3d end-to-end verification
  before release.
- **Control plane** — `ravn-server`: ingestion, storage, policy, approvals, API.
- **Portal** — the web UI: inventory, live messages, remediation approvals and
  audit, topology.

Full detail in [docs/architecture.md](docs/architecture.md).

## Roadmap

What, how and when are laid out in [docs/roadmap.md](docs/roadmap.md) — including
which parts of self-healing are shipped versus still in progress.

## Get involved

Ravn is MIT-licensed and we'd love help. Good places to start:

- Browse issues labelled [`good first issue`](https://github.com/olafkfreund/ravn-agents/labels/good%20first%20issue)
  and [`help wanted`](https://github.com/olafkfreund/ravn-agents/labels/help%20wanted).
- Read [CONTRIBUTING.md](CONTRIBUTING.md) for dev setup (Rust workspace, Nix
  devshell, portal with pnpm).
- Join the conversation in [Discussions](https://github.com/olafkfreund/ravn-agents/discussions) —
  questions, ideas, and the public devlog where we post progress and struggles.
- Follow the blog on our [GitHub Pages site](https://olafkfreund.github.io/ravn-agents/).

Whether you write Rust, React, run odd hardware, or just have logs that would make
a good test fixture — there's a way in.

## License

[MIT](LICENSE) © 2026 Olaf Krasicki-Freund and the Ravn contributors.
