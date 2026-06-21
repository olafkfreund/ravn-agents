---
title: "Production hardening, and the bugs that only showed up when we combined the branches"
description: "M6 lands: durable audit, an in-cluster K8s executor, signing-key rotation, self-observability, air-gapped install — and four real bugs that no single PR review could have caught."
author: "Olaf"
tags: ["devlog", "m6", "remediation", "kubernetes", "decision", "retro"]
---

The M6 hardening milestone is on `main`. The headline is unglamorous on purpose:
take the self-healing loop from "works in the demo" to "you could run this in a
regulated, air-gapped shop and trust the audit trail." Fifteen issues, one epic
([#158](https://github.com/olafkfreund/ravn-agents/issues/158)).

The interesting part isn't the feature list. It's that **four of the bugs we
found don't exist in any single branch** — they only appeared when the branches
were combined. That's the honest argument for integrating deliberately instead
of merging fifteen green PRs and hoping.

## What shipped

- **Durable audit trail.** The remediation record (proposal → decision → command
  → result) now lives in append-only Postgres, not memory. "Auditable" stops
  being a slide when the record survives a control-plane restart. ([#143](https://github.com/olafkfreund/ravn-agents/issues/143))
- **An in-cluster Kubernetes executor.** The pod-restart templates finally have
  something that runs them: a component that pulls signed commands, **re-verifies
  the Ed25519 signature in-cluster**, and executes typed capabilities
  (`delete_pod`, `restart_deployment`, `pod_state`) under least-privilege,
  namespaced RBAC. ([#146](https://github.com/olafkfreund/ravn-agents/issues/146))
- **Signing-key rotation** with versioned keys and an overlap window — rotate the
  command-signing key with no fleet re-enrollment. ([#150](https://github.com/olafkfreund/ravn-agents/issues/150))
- **Self-observability.** Prometheus `/metrics` and a Ravn event when the policy
  circuit breaker trips. The monitoring tool now monitors itself. ([#149](https://github.com/olafkfreund/ravn-agents/issues/149))
- **Air-gapped install**, a non-NixOS host quickstart, a Helm chart, a real
  `ravn-eval` benchmark harness, and an MCP server — the install story is now
  first-class on host, Kubernetes, and fully offline.

Through all of it, the founding rule is unchanged: **the LLM is never in the
detection or action path.** It writes the explanation; deterministic code and
signed templates decide and act.

## The bugs that hid

These are the useful part — each is invisible until two branches meet.

- **The guard that rejected its own executor.** The in-cluster executor runs
  commands *through* the shared `handle_command`. One branch added a guard there
  that rejects Kubernetes capabilities — so the executor would have rejected
  every command it exists to run. Its own unit tests passed, because they test
  the guard and the executor *separately*, never composed. We moved the guard to
  the host actuator's socket boundary, where it belongs.

- **Two designs for one enum.** Two branches independently invented the K8s
  capability type — different variant names, different fields. Both compiled.
  Reconciling them to one canonical design was a deliberate call, not a textual
  merge.

- **A typed-path check that caught a real template typo.** New startup validation
  of template condition paths immediately rejected a shipped K8s template that
  used `reason` instead of `payload.reason` — a template that, before the check,
  would have silently *never matched*.

- **A `SIGKILL` that produced no proposal.** This one only surfaced in the NixOS
  VM end-to-end test. The test kills a unit with `SIGKILL`; systemd reports the
  result as `signal`. But the generic restart template had been narrowed to match
  only `result = "exit-code"`. So the unit failed, and Ravn proposed *nothing* —
  the worst kind of failure, the silent one. The fix was to match any failed unit
  regardless of result, and to lock it in with a test that fails the moment the
  catch-all gets narrowed again.

Every one of those passed `cargo test` on its own branch. The VM test — boot a
real NixOS machine, kill a unit, assert a signed heal record appears — is the only
thing that catches the last one. Slow tests earn their keep.

## A smaller, sharper one: "is it broken, or just deploying?"

A question we kept getting: how does Ravn avoid "healing" something that's just
being restarted or redeployed? Most of the answer is state semantics — systemd's
`failed` state and Kubernetes' failure *reasons* don't fire for clean restarts or
rollouts. But a unit can briefly enter `failed` before systemd's own `Restart=`
recovers it. So the failed-unit tap now has a **grace period**: a unit must stay
failed for a configurable window (default 15s) before it emits anything. Brief
blip, recovered in time → no proposal, no noise. ([#164](https://github.com/olafkfreund/ravn-agents/issues/164))

## What's next

The code is on `main` and green; the cross-target *end-to-end* proofs (k3d heal,
air-gapped VM, live-Postgres restart) are running their way through CI now, and
that's the bar for calling the epic done — not "it compiles." There's also a
copy-paste [remediation template library](https://github.com/olafkfreund/ravn-agents/tree/main/examples/remediation-templates)
now, so adding a heal for *your* service is a TOML file, not a code change.

As always, built in the open. The bugs above are in the git history, not hidden
in a changelog — that's the whole point.
