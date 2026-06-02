---
title: "Hello from Ravn"
description: "Why we're building a fleet of small local-LLM agents to watch Linux servers — and building it in the open."
pubDate: 2026-06-02
author: "Olaf"
tags: ["devlog", "intro"]
---

Ravn started as a simple question: could you bolt a tiny language model onto a
Linux server — CPU only, no GPU — and have it review logs and services, then tell
you in plain words when something's wrong?

The short answer is yes, with one important twist. The model should **not** be the
thing that decides whether something is wrong. Deterministic tooling already does
that job well — journald, failed-unit state, config diffs, auth events. What's
missing is the *last mile*: turning a flagged event into a clear sentence a human
can act on. That's the job we're handing to a small model (think Qwen3 1.7B), and
it's a job a CPU handles comfortably. If the model is slow or wrong, you still get
the alarm — you just get a worse explanation. Safe failure.

From there it grew. One agent became a fleet. A fleet needs somewhere to report to,
so there's a control plane. And a fleet you can't *see* isn't much use, so there's a
portal: inventory, a live message feed, and a topology view where you group your
agents into whatever categories make sense to you — environment, role, business unit,
location.

We're building this in the open, MIT-licensed, because it's the kind of tool that
gets better with other people's weird hardware, odd log formats, and hard-won
operational scars. The [roadmap](/ravn-agents/roadmap/) lays out what, how and when,
from a walking skeleton (M0) through to alert routing and an eval harness (M5).

This blog is the devlog. Expect progress, design decisions, and — honestly — the
struggles too. The dead ends are usually the useful part. Comments are open on every
post (they're backed by GitHub Discussions), so if you've solved something we're
stuck on, or just want to argue about transport protocols, jump in.

If you want to get involved, the [issues](https://github.com/olafkfreund/ravn-agents/issues)
are organised by epic with `good first issue` labels, and
[Discussions](https://github.com/olafkfreund/ravn-agents/discussions) is open for ideas.

More soon. 🐦‍⬛
