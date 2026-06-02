# Ravn

**Small local-LLM agents that watch your Linux servers and report back in plain language.**

Ravn is a fleet of lightweight agents — one per host — that watch logs, services,
network, access, config drift and updates. Detection is deterministic and fast; a
small CPU-only language model (e.g. Qwen3 1.7B) runs the *last mile*: it turns a
flagged event into a clear, human-readable explanation and a suggested next check.
A central control plane collects everything, and a modern web portal gives you fleet
inventory, a live message feed, and a category-grouped topology view of your agents.

Named for the raven — Odin's scouts that fly out across the world and return to
tell him what they saw.

> Status: early development. We're building in the open. See the
> [Roadmap](docs/roadmap.md) and come say hello in
> [Discussions](https://github.com/olafkfreund/ravn-agents/discussions).

## Why Ravn

- **The LLM is never in the detection hot path.** Deterministic tooling decides
  *whether* something is wrong and fires the alarm. The model only writes the
  *explanation*. A slow or wrong model degrades the wording, never the alerting.
- **CPU-first.** Sub-2B models run comfortably on a server CPU. No GPU required,
  near-zero idle cost — small enough to run on every box, not just one.
- **Self-hosted and declarative.** Single static binaries, first-class NixOS
  modules, OCI images for everywhere else.

## Architecture at a glance

Three planes:

- **Edge** — `ravnd`, the agent on each host: detection taps + local inference.
- **Control plane** — `ravn-server`: ingestion, storage, API.
- **Portal** — the web UI: inventory, live messages, topology showcase.

Full detail in [docs/architecture.md](docs/architecture.md).

## Roadmap

What, how and when are laid out in [docs/roadmap.md](docs/roadmap.md), milestones
M0 (walking skeleton) through M5 (alert routing + eval harness).

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
