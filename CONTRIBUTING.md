# Contributing to Ravn

Thanks for considering a contribution. Ravn is built in the open and welcomes
issues, ideas, docs, test fixtures and code.

## Ways to help

- **Code** — pick up a [`good first issue`](https://github.com/olafkfreund/ravn-agents/labels/good%20first%20issue)
  or anything under an open [epic](https://github.com/olafkfreund/ravn-agents/labels/epic).
- **Test fixtures** — real (sanitised) log snippets that should or shouldn't trigger
  an alert are gold. Drop them in `tests/fixtures/` with a short note.
- **Docs & devlog** — improve these docs, or share a deployment story on the blog.
- **Discussion** — design feedback in [Discussions](https://github.com/olafkfreund/ravn-agents/discussions)
  before large changes saves everyone time.

## Project layout

```
ravn-agents/
├── crates/
│   ├── ravn-core/     # shared types + event schema
│   ├── ravn-agent/    # ravnd: detection + local inference
│   └── ravn-server/   # control plane: ingestion, API, storage
├── portal/            # React + TS web UI
├── site/              # GitHub Pages site (plan + blog)
├── nix/               # flake + NixOS modules
└── docs/              # architecture, roadmap
```

## Dev setup

A Nix flake provides the full toolchain:

```bash
nix develop          # Rust, NATS, Postgres, node/pnpm, llama.cpp
```

Without Nix:

- Rust (stable, via rustup) for the workspace: `cargo build`
- PostgreSQL 15+ and a NATS server for the control plane
- Node 20+ and pnpm for the portal: `cd portal && pnpm install && pnpm dev`
- A `llama-server` (llama.cpp) with a small GGUF model for agent inference

The fastest end-to-end loop is milestone **M0** (see [ROADMAP](docs/roadmap.md)):
agent emits an event → NATS → server persists → portal lists it.

## Pull requests

- Branch from `main`, keep PRs focused, reference the issue (`Closes #123`).
- Run `cargo fmt`, `cargo clippy`, and `pnpm lint` before pushing.
- Conventional-commit style messages (`feat:`, `fix:`, `docs:` …) are appreciated.
- New behaviour should come with a test or a fixture.

## Code of conduct

Be decent. We follow the [Contributor Covenant](https://www.contributor-covenant.org/).
Report concerns via a private message to the maintainers.
