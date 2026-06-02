---
layout: page
title: Get Involved
permalink: /get-involved/
---

Ravn is MIT-licensed and built in the open. Help of every kind is welcome — code,
docs, test fixtures, or just sharp questions.

## Where to start

- **Pick an issue** — browse [`good first issue`](https://github.com/olafkfreund/ravn-agents/labels/good%20first%20issue)
  and [`help wanted`](https://github.com/olafkfreund/ravn-agents/labels/help%20wanted),
  or anything under an open [epic](https://github.com/olafkfreund/ravn-agents/labels/epic).
- **Read** [CONTRIBUTING.md](https://github.com/olafkfreund/ravn-agents/blob/main/CONTRIBUTING.md)
  for dev setup (Rust workspace, Nix devshell, portal with pnpm).
- **Talk** in [Discussions](https://github.com/olafkfreund/ravn-agents/discussions) —
  design ideas, questions, and the place comments on these posts land.

## Good contributions for newcomers

- **Test fixtures** — real (sanitised) log snippets that should or shouldn't trigger
  an alert. These directly improve the model prompts and regression tests.
- **Detection taps** — a new source of events (a daemon's logs, a package manager)
  following the `ravn-core` event schema.
- **Docs & devlog** — clarify setup, or write up a deployment story.

## How decisions happen

Larger changes start as a Discussion or an issue before code, so we can agree the
shape. Epics track the big pieces; child issues are the bite-sized work. Comment on
an issue to claim it.
