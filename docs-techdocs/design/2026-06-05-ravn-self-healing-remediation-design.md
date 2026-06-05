# Design: Ravn Self-Healing Remediation

> Created: 2026-06-05
> Status: Approved (design) — not yet planned/implemented
> Scope: Turning Ravn from a read-only observer into a supervised, self-healing
> actor on Linux/NixOS hosts, using curated deterministic remediation templates,
> a signed command channel, declarative policy, and a per-environment knowledge base.

## Summary

Ravn today **detects and explains** — agents are detection-only, connect
outbound-only, and hold read-only privilege; the small local model writes the
*explanation* and a *suggested check*, never an action. This design adds a
**second loop** on top of that output: **find → suggest → resolve** (after a
human accepts, or as a policy-approved auto-process), and **learn** from every
fix so the next occurrence is handled faster.

The design is the mirror image of Ravn's founding doctrine. Detection's anchor is
*"the LLM is never in the detection hot path; a wrong model degrades the wording,
never the alerting."* Remediation's anchor is its reflection:

> **The LLM is never in the action hot path either.** It only proposes *which
> pre-authored, deterministic remediation template* to run and with what
> parameters. A human or a signed policy approves. Deterministic, least-privilege,
> sandboxed code executes. A wrong model proposes a bad *suggestion* — caught by
> policy or a human — never a bad *action*.

This keeps Ravn's selling point intact: a small CPU-only model is safe to use
even for remediation, because it never touches execution.

The four cooperating subsystems — the remediation engine (PARR loop), the secure
command channel, the policy/security model, and the knowledge base — are designed
as **one closed loop** because they cannot be sensibly built apart. `.deb`/`.rpm`
packaging is a separate, independent effort and is **out of scope** for this spec.

## Background & research

Prior art that shaped the design (sources at the end):

- **Risk-tiered autonomy is the universal pattern.** Low-risk actions (restart a
  stalled unit) auto-execute; high-risk actions (anything touching data or infra
  state) pause for human-in-the-loop (HITL) approval. The EU AI Act codifies
  "human on the loop" plus a full audit trail for high-risk actions.
- **Runbooks vs. playbooks.** Runbooks are step-by-step remediations for a
  *specific* fault; playbooks are higher-level decision trees with approval gates.
  This is the unit the knowledge base accretes — a curated template *is* a runbook.
- **Policy-as-code gates every action** (OPA/Kyverno-style): reject non-compliant
  remediations and force a compliant alternative. We adopt the *pattern*
  (declarative, default-deny, signed) without the heavyweight engine.
- **The LLM must never execute directly.** Consensus secure pattern: the model may
  only *select a pre-approved plan* and *fill parameters*; deterministic code
  executes; least-privilege per step; sandboxed; every call logged. Over-privileged
  agents are exploited at ~85% success rates — privilege separation is non-negotiable.
- **Closed loop = detect → diagnose → remediate → learn**, where "learn" feeds the
  next decision. This is exactly the user's Prepare/Act/Reflect/Review (PARR) cycle.

## Decisions (locked in the brainstorm)

| #  | Decision | Choice |
|----|----------|--------|
| D1 | Scope | Self-healing loop (engine + PARR + channel + security + KB) as one spec; `.deb`/`.rpm` packaging split off |
| D2 | Trust model | **Curated templates only** — the LLM selects a template and fills validated params; it never emits commands |
| D3 | Execution surface | **Host (Linux + NixOS) first**; NixOS generation rollback is a first-class heal primitive |
| D4 | Command channel | **Agent pulls** approved actions over its existing outbound connection; each command is **Ed25519-signed** by the control plane and verified by the agent |
| D5 | Policy / autonomy | Declarative per-template **risk tiers** + per-env **signed policy**, **default-deny**, deterministic evaluation in the control plane |
| D6 | Knowledge base | Per-env **markdown wiki in git** + structured front-matter for **deterministic recall** that biases the next suggestion (no embedding model) |
| D7 | Action form | **Typed, whitelisted capabilities** implemented in audited Rust; templates compose them |
| D8 | Privilege model | **Privilege separation** — a tiny privileged `ravn-actuator` executes capabilities; `ravnd` stays unprivileged |
| D9 | Spec location | Non-published `plans/` (repo `docs/` is the Jekyll site), consistent with the K8s design doc |

## Architecture

Detection (today) produces a `ravn-core::Message`. Remediation bolts a **PARR
loop** onto that output and never touches the detection hot path.

```
   DETECT (exists today)            PARR REMEDIATION LOOP (new)
   ┌──────────────┐
   │ ravnd taps   │  Message   ┌──────────────────────────────────────────────┐
   │ → Event      │──────────► │ PREPARE   (control plane, off hot path)        │
   └──────────────┘            │  1. recall: fault signature → KB + templates   │
                               │  2. LLM proposes: template + params + rationale│
                               │  3. policy eval: risk×scope → auto|approve|deny │
                               │  → RemediationProposal                         │
                               └───────────────┬────────────────────────────────┘
                                  auto │         │ approve → human queue (portal/sink)
                                       ▼         ▼ (on approve)
                               ┌──────────────────────────────────────────────┐
                               │ ACT                                            │
                               │  control plane SIGNS a CommandEnvelope         │
                               │  ravnd PULLS it (outbound), verifies signature │
                               │  → hands typed-capability call to ravn-actuator│
                               │  → actuator (root) runs ONLY a whitelisted op  │
                               └───────────────┬────────────────────────────────┘
                                               ▼
                               ┌──────────────────────────────────────────────┐
                               │ REFLECT  (agent → control plane)               │
                               │  run template's deterministic verify check     │
                               │  capture before/after + outcome                │
                               │  fail → rollback (template or nix generation)  │
                               │         + escalate to human                    │
                               └───────────────┬────────────────────────────────┘
                                               ▼
                               ┌──────────────────────────────────────────────┐
                               │ REVIEW  (control plane)                        │
                               │  immutable audit record (who/what/sig/outcome) │
                               │  LLM drafts retrospective .md → human edits     │
                               │  commit to per-env KB git + update recall index│
                               │  novel fault, no template → KB "gap" + author  │
                               └──────────────────────────────────────────────┘
```

Every arrow into a host is **pulled** and **signed**; every action is a **typed
capability** run by the **privileged actuator**, never by the model or the
network-facing process.

### Components

- **`ravn-actuator`** (new crate, host) — the *only* privileged component. Exposes
  the fixed typed-capability set over a local Unix socket; verifies peer
  credentials, re-validates the signed envelope and the params, executes, returns a
  structured `ActionResult`. No network, no model, minimal dependencies — a small,
  auditable trusted computing base (TCB). It is **intentionally a standalone crate**
  (not a feature-gated module in `ravn-agent`) so its dependency surface stays small
  and independently auditable — the same build-profile reasoning the K8s design
  applied to `kube-rs`.
- **`ravnd`** (extended) — pulls signed `CommandEnvelope`s over its existing
  outbound connection, verifies the control-plane signature against a pinned key,
  calls the actuator, runs the verify check, reports the `ActionResult`. Stays
  **unprivileged**.
- **`ravn-server`** (extended) — the PARR orchestrator: recall, LLM proposal,
  policy evaluation, approval queue, command signing, audit store, retrospective/KB
  writer.
- **`ravn-core`** (extended) — new shared types: `Template`, `RiskTier`,
  `Capability` (+ typed params), signed `CommandEnvelope`, `ActionResult`,
  `RemediationProposal`, `RemediationRecord`. Additive and backward-compatible.
- **Three git-backed stores** — the template library, the per-env signed policy,
  and the per-env knowledge-base wiki.
- **Portal** (extended) — an approval queue and an action audit timeline.

## PARR loop — phase by phase

- **Prepare** (control plane, off the hot path): an Event/Message arrives from the
  existing pipeline. **Recall** matches the fault signature against the KB and the
  template library. The **LLM proposes** a template + parameters + a human-readable
  rationale and a confidence. **Policy evaluation** maps `{risk tier × scope}` to
  `auto | approve | forbid`. Output: a `RemediationProposal`.
- **Act**: `auto` → the control plane signs a `CommandEnvelope` immediately;
  `approve` → it enters the human queue and is signed on approval. Either way, the
  agent pulls the signed envelope, verifies it, and the **actuator** runs the typed
  capabilities. Act is always deterministic.
- **Reflect** (agent → control plane): run the template's deterministic **verify**
  post-condition; capture before/after state and outcome. Verify fails → declared
  rollback (or NixOS generation rollback); rollback fails → freeze the target and
  escalate.
- **Review** (control plane): write the immutable audit record; the LLM drafts a
  retrospective markdown entry; a human may edit; commit to the per-env KB git and
  update the recall index. A novel fault with no matching template writes a `gap`
  entry and opens a tracker issue.

## Security & safety model

Seven layers, each defending a specific threat.

1. **Command integrity — signed envelopes.** The control plane holds an **Ed25519**
   signing keypair; each agent pins the control-plane public key at enrollment.
   ```
   CommandEnvelope { command_id, agent_id, template_id+version, capability,
                     validated_params, risk_tier,
                     approval_ref (oidc-user+ts | "policy:auto"),
                     nonce, issued_at, expires_at, sig }
   ```
   `ravnd` verifies the signature before acting. **Replay protection:** nonce +
   short expiry + an agent-side executed-`command_id` ledger (also gives
   idempotency). A compromised broker can neither forge nor replay an action.
2. **Transport auth (closes the known gap).** Per-agent connection credentials —
   NATS nkey/JWT or mTLS, enrolled with bootstrap tokens (roadmap M4's direction,
   now load-bearing). Each agent is authorized only for its own commands subject
   (`ravn.<agent_id>.commands`, mirroring the existing `ravn.messages.<agent_id>`
   publish subject — the final scheme is reconciled with the shared transport-auth
   epic, which the K8s design also depends on). Signing sits on top, **independent
   of broker trust**: because envelope verification needs only the pinned
   control-plane public key, the P1 walking skeleton can land before per-agent
   transport auth (#26) does — P1 is not hard-blocked on it.
3. **Policy & risk tiers (default-deny).** Three risk classes declared per template:
   `safe` (idempotent, no data loss), `guarded` (service-affecting but reversible),
   `dangerous` (potential data loss / wide blast). The per-env **signed** policy
   maps `{tier × scope(host/category/unit)} → {auto | approve | forbid}`,
   default-deny. The policy file is signed with the same trust root, so a tampered
   policy cannot silently widen auto-execution. **Global guardrails:** per-host rate
   limit + circuit breaker (repeated same-fault auto-action escalates to a human —
   kills restart storms/flapping) and a fleet-wide kill switch. `dangerous` never
   auto-executes.
4. **Privsep actuator boundary.** `ravn-actuator` runs privileged via its own
   hardened systemd unit; `ravnd` connects over a local Unix socket with a
   **peer-credential check** (only ravnd's uid may connect). The actuator
   **independently re-validates**: capability ∈ whitelist, params pass the typed
   schema, *and* re-verifies the signed envelope — so even a compromised `ravnd`
   cannot fabricate a capability call.
5. **Pre/post conditions — verify & rollback.** Each template declares
   **preconditions** checked before Act (e.g. "only if unit is `failed`") and a
   deterministic **verify** post-condition checked after. Verify fails → declared
   rollback, or NixOS generation rollback as the universal net; rollback fails →
   freeze target + critical escalation.
6. **Audit trail.** Append-only `RemediationRecord` in Postgres for every proposal
   → decision → approval → execution → verification → rollback, capturing the
   signature, the **OIDC approver identity** (reusing the existing Authelia/Keycloak
   auth) or `policy:auto`, and full before/after state. Surfaced in the portal and
   exportable.
7. **Threat model.**

   | Threat | Defense |
   |--------|---------|
   | Compromised broker | Ed25519 signing + replay protection |
   | Compromised `ravnd` | Privsep + actuator re-validation + fixed capability set |
   | Hallucinated / malicious LLM proposal | Policy gate + human approval + deterministic verify |
   | Tampered policy / template / KB | Git review + signing of policy & commands |
   | Restart storm / flapping | Rate limit + circuit breaker + escalation |
   | Replay | Nonce + expiry + idempotency ledger |

## Templates

A template is a TOML descriptor in git, human-authored and reviewed. It composes
typed capabilities; it never contains shell.

```toml
id = "failed-unit-restart"
version = 3
title = "Restart a failed systemd unit"
risk_tier = "safe"

[match]                      # fault signature → candidate template
source = "FailedUnit"
conditions = { active_state = "failed" }

[parameters]
unit = { type = "string", from = "payload.unit" }   # typed, validated

preconditions = [ { capability = "unit_state", equals = "failed" } ]
steps        = [ { capability = "reset_failed", unit = "{{unit}}" },
                 { capability = "restart_unit",  unit = "{{unit}}" } ]
verify       = { capability = "unit_state", equals = "active", timeout_s = 30 }
rollback     = "none"        # safe/idempotent; nix_generation rollback is the universal net
```

### Capability catalog v1

Each capability is audited Rust in `ravn-actuator`, with the narrowest privilege
that suffices. New capabilities require a Ravn release (code review); templates
that *compose* existing capabilities are user-authorable in git.

- `restart_unit`, `reset_failed`, `reload_config`
- `prune_journal` (journald vacuum), `truncate_logfile` (path-allowlisted)
- `kill_process` (criteria-bounded)
- `nix_rollback` (to a previous generation)
- read-only checks: `unit_state`, `disk_usage`

## Knowledge base

A per-environment git directory of markdown files, one per fault pattern, with
structured front-matter that drives deterministic recall.

```markdown
---
fault_signature: "FailedUnit:nginx.service:failed"
template_used: failed-unit-restart@3
params: { unit: nginx.service }
outcomes: [{ ts: 2026-06-05T10:02Z, result: success, ttr_s: 8, approver: "policy:auto" }]
occurrences: 14
---
# nginx.service repeatedly enters failed state
**Reflect:** restart resolves it; recurrence suggests an upstream cause (see incident notes).
```

- **Recall is deterministic:** a new fault matches `fault_signature`/keywords →
  ranked past resolutions are injected into the LLM's proposal prompt ("last 14×
  this fired, `failed-unit-restart` worked, avg 8s"). No embedding model, no vector
  store — CPU-light and in-doctrine. The semantic/vector path can be added later
  behind the same recall interface if needed.
- **Gap tracking:** a novel fault with no matching template writes a `gap` entry and
  opens a tracker issue prompting a human to author a template — the only path by
  which the catalog grows.

## Code structure

- **new** `crates/ravn-actuator` — privileged executor, local socket, capability
  implementations.
- `crates/ravn-agent` — a `remediation/` module: pull, verify signature, call the
  actuator, run verify, report.
- `crates/ravn-server` — a `remediation/` module: recall, LLM proposal, policy
  evaluation, approval queue, signing, audit, KB writer.
- `crates/ravn-core` — the new types and the signed-envelope schema (additive).
- git stores: `templates/`, `policy/<env>.toml`, and the per-env KB directory.
- `nixos/modules/agent.nix` — a hardened `ravn-actuator` systemd unit and
  `services.ravn.agent.remediation` options.
- `portal/` — an approval-queue page and an action audit timeline.

## What we deliberately do NOT do

- No LLM-generated commands or scripts — ever.
- No inbound network port on agents; the command channel is pull-only.
- No arbitrary shell; only typed, whitelisted capabilities.
- No cross-environment policy sharing; each env has its own signed policy.
- No Kubernetes remediation in v1 (host first; k8s is a later surface).
- No auto-execution of the `dangerous` tier.

## Phasing

- **P1 — Remediation walking skeleton.** Core schema + `ravn-actuator` + one
  capability + the signed pull channel + **manual-approval-only** end-to-end (the
  `failed-unit-restart` template) + the audit record + the portal approval queue.
- **P2 — Autonomy.** Policy/risk tiers + auto-execute + circuit breaker + rate
  limit + fleet-wide kill switch.
- **P3 — Safety net.** Pre/post-condition verify + rollback + NixOS generation
  rollback + target freeze on rollback failure.
- **P4 — Knowledge base.** Retrospective writer + deterministic recall + gap
  tracking.
- **P5 — Breadth.** More capabilities/templates, approval via alert sinks
  (Slack/ntfy action), and the Kubernetes execution surface.

## Testing strategy

- **Unit tests:** template validation; policy evaluation (default-deny, scope
  matching); signature and replay verification; capability param validation
  (allowlists). Runnable under `nix flake check`.
- **Actuator tests:** each capability against a real systemd in a container/VM.
- **NixOS VM E2E:** inject a failed unit → assert a proposal is produced → approve
  → assert the actuator restarts the unit → assert verify sees `active` → assert an
  audit row with the signature and approver.
- **Negative/security tests:** forged signature rejected; expired/replayed envelope
  rejected; policy `forbid` produces no action; circuit breaker trips after N
  repeats; a compromised-`ravnd` simulation cannot invoke a non-whitelisted op.

## Tracker

Create a new epic **"Ravn Self-Healing Remediation"** with child issues:

1. `ravn-core`: remediation schema — `Template`, `RiskTier`, `Capability` (+ typed
   params), signed `CommandEnvelope`, `ActionResult`, `RemediationProposal`,
   `RemediationRecord`.
2. `ravn-actuator`: new privileged executor crate — local socket, peer-cred check,
   envelope re-verification, capability catalog v1.
3. Signed command channel — control-plane Ed25519 signing + key pinning at
   enrollment; `ravnd` pull + verify + idempotency ledger; per-agent
   `ravn.<agent_id>.commands` authorization (links the auth epic).
4. Control-plane remediation orchestrator — Prepare (recall + LLM proposal),
   approval queue, signing, `RemediationRecord` audit store.
5. Policy engine — declarative risk tiers + per-env signed policy + default-deny
   evaluation + rate limit / circuit breaker / kill switch.
6. Verify & rollback — pre/post conditions, template rollback, NixOS generation
   rollback, target freeze on rollback failure.
7. Knowledge base — retrospective writer, deterministic recall, gap tracking +
   issue creation.
8. Portal — approval queue page + action audit timeline.
9. NixOS module — hardened `ravn-actuator` unit + `services.ravn.agent.remediation`.
10. E2E — NixOS VM test (inject failed unit → propose → approve → heal → verify →
    audit) + security negative tests.

## Open questions / future

- **Approval via alert sinks** (Slack/ntfy actionable buttons) — deferred to P5;
  portal-first in P1–P4.
- **Multi-approver for `dangerous`** (two-person rule) — a candidate hardening once
  the single-approver flow is proven.
- **Kubernetes execution surface** — rollout restart, delete crashlooping pod,
  cordon/drain; reuses the proposal/policy/audit core with a k8s capability set.
- **Vector/semantic recall** — behind the same recall interface, if deterministic
  signature matching proves too narrow.
- **Break-glass / disable-all** procedure and its audit treatment.

## Sources

- [Agentic SRE / self-healing AIOps in 2026 — Unite.AI](https://www.unite.ai/agentic-sre-how-self-healing-infrastructure-is-redefining-enterprise-aiops-in-2026/)
- [Agentic remediation guide — BigID](https://bigid.com/blog/agentic-remediation-guide/)
- [Self-healing infrastructure with agentic AI — Algomox](https://www.algomox.com/resources/blog/self_healing_infrastructure_with_agentic_ai/)
- [Runbook automation tools 2026 — incident.io](https://incident.io/blog/runbook-automation-tools-2026-the-complete-guide)
- [Agentic runbooks for Kubernetes — Cast AI](https://cast.ai/blog/agentic-runbooks/)
- [Policy-as-code with OPA — env0](https://www.env0.com/blog/how-policy-as-code-enhances-infrastructure-governance-with-open-policy-agent-opa)
- [Insecure tool use & function calls — Sourcery](https://www.sourcery.ai/security/categories/insecure_tool_calls)
- [LLM agents should employ security principles — arXiv 2505.24019](https://arxiv.org/pdf/2505.24019)
- [AI agent sandbox (Firecracker/gVisor isolation) — Firecrawl](https://www.firecrawl.dev/blog/ai-agent-sandbox)
- [Agentic AIOps guardrails — VKTR](https://www.vktr.com/ai-technology/agentic-aiops-building-the-guardrails-for-autonomous-infrastructure/)
