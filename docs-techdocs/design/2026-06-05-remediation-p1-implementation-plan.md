# Implementation Plan: Remediation Phase 1 — Walking Skeleton

> Created: 2026-06-05
> Status: Ready for Implementation
> Design spec: `plans/2026-06-05-ravn-self-healing-remediation-design.md`
> Epic: #122 · Covers: #112, #113, #114, #115, #119, #120 (+ partial #121)

## Goal

One real thread through the whole remediation loop, **manual-approval-only**: a
`FailedUnit` event produces a proposal to run the `failed-unit-restart` template;
an operator approves it in the portal; the control plane signs a `CommandEnvelope`;
`ravnd` pulls it, verifies the signature, and the privileged `ravn-actuator`
restarts the unit; the result is reported and written to an immutable audit record.

This proves the shape — schema, signing, pull channel, privsep execution, approval
UI, NixOS packaging — that P2–P5 build on.

## Success criteria (definition of done)

- [ ] On a NixOS VM, a deliberately-failed unit yields a pending proposal in the portal.
- [ ] Approving it causes the unit to return to `active`, executed by `ravn-actuator` (not `ravnd`).
- [ ] A `RemediationRecord` row captures proposal → approval (OIDC user) → execution → result, including the command signature.
- [ ] A forged or expired `CommandEnvelope` is **rejected** by the agent and never executed.
- [ ] `ravnd` runs unprivileged; only `ravn-actuator` holds privilege.
- [ ] `nix flake check` is green (unit + the VM E2E).

## Out of scope for P1 (deferred — do NOT build here)

- Policy engine / risk tiers / auto-execute (#116, P2) — **P1 always routes to human approval.**
- Pre/post-condition framework, rollback, target freeze (#117, P3) — P1 actuator returns the resulting `unit_state` only; no automatic rollback.
- Knowledge base + recall (#118, P4) — P1 has no recall; the proposal is produced by a direct fault→template match.
- Additional capabilities/templates, alert-sink approval, K8s surface (P5).
- NATS request/reply command transport — **P1 uses HTTPS long-poll** (see Decision 1).

## Grounding (current code the plan builds on)

- **Transport is publish-only** — `crates/ravn-agent/src/transport.rs`: `Transport::{Nats,Ws}` expose only `publish`/`publish_heartbeat`. There is no inbound/pull path; the command channel is net-new.
- **mTLS enrollment exists** — `crates/ravn-agent/src/enrollment.rs` + `crates/ravn-core/src/enrollment.rs`: the agent already holds `agent.key`/`agent.crt`/`ca.crt`, and `EnrollResponse` is the natural place to deliver the signing pubkey.
- **Server is Axum** with `/enroll`, ingest, auth, CA modules (`crates/ravn-server/src/`), Postgres via SQLx — the command endpoints and audit table slot in here.
- **Core schema** — `crates/ravn-core/src/`: `Message`/`Event`/`Payload`, with `Source::FailedUnit` + `FailedUnitPayload { unit, .. }`, the exact fault the P1 template matches.

## Key P1 implementation decisions

1. **Command pull = mTLS-authenticated HTTPS long-poll** (not NATS, not inbound).
   - `GET /agents/{agent_id}/commands` (long-poll, mTLS client-cert auth) → returns pending signed envelopes.
   - `POST /agents/{agent_id}/commands/{command_id}/result` → agent reports `ActionResult`.
   - **Why:** reuses the cert the agent already has, preserves outbound-only, and avoids depending on the unbuilt per-agent NATS auth (#26). NATS request/reply is a later refactor behind the same `CommandSource` trait.
2. **Signing key delivery = piggyback on enrollment.** Add `command_signing_pubkey: String` (Ed25519, PEM/base64) to `EnrollResponse`; the agent pins it alongside the CA. Control plane generates/loads the keypair at startup (private key in a config-referenced file/secret, `0600`).
3. **Crate `ed25519-dalek`** for sign/verify (well-maintained, pure-Rust, no_std-friendly).
4. **Actuator IPC = Unix domain socket** at a fixed path (e.g. `/run/ravn/actuator.sock`), `SO_PEERCRED` check that the connecting uid is `ravnd`'s. Line-delimited JSON request/response of `{CommandEnvelope → ActionResult}`; the actuator **re-verifies the signature** independently.

## Execution order (dependency graph)

```
#112 ravn-core schema  ──►  #113 ravn-actuator ──┐
        │                   #114 signing+pull ───┼──► #121(E2E, partial) ──► VM test
        └──► #115 orchestrator ──► #119 portal ──┘
                                   #120 nixos module (after #113 + ravnd remediation wiring)
```
Build #112 first (everything imports it). #113 and #114 can proceed in parallel
once the schema lands; #115 needs #114's signing; #119 needs #115's API; #120 needs
#113's binary + #114's agent-side wiring; #121 ties it together.

## Tasks

### 1. `ravn-core`: remediation schema (#112)

- [ ] 1.1 Write tests: serde round-trip + JSON-schema generation for every new type; a backward-compat test that a pre-remediation `Message` still deserializes.
- [ ] 1.2 Add `RiskTier { Safe, Guarded, Dangerous }` and `Capability` (enum with typed params; P1 needs `ResetFailed{unit}`, `RestartUnit{unit}`, `UnitState{unit}`).
- [ ] 1.3 Add `Template` (id, version, title, risk_tier, match, parameters, steps, verify) — deserialized from TOML; include a loader + validation (param refs resolve, capabilities exist).
- [ ] 1.4 Add `CommandEnvelope { command_id, agent_id, template_id, template_version, capability, params, risk_tier, approval_ref, nonce, issued_at, expires_at, sig }` with `sign(&signing_key)` / `verify(&pubkey)` helpers (canonical serialization for the signed bytes).
- [ ] 1.5 Add `ActionResult { command_id, status: Succeeded|Failed|Rejected, detail, observed_state, finished_at }` and `RemediationProposal` / `RemediationRecord`.
- [ ] 1.6 Export from `lib.rs`; regenerate `schema/message.schema.json`; verify all tests pass.

### 2. `ravn-actuator`: privileged executor (#113)

- [ ] 2.1 Write tests: capability param validation (rejects unknown unit chars / empties); signature re-verification rejects a tampered envelope; peer-cred check rejects a non-`ravnd` uid (mock).
- [ ] 2.2 Scaffold the new crate (binary). Minimal deps: `ravn-core`, `tokio`, `serde_json`, `ed25519-dalek`, `nix`/`libc` for `SO_PEERCRED`.
- [ ] 2.3 Implement the Unix-socket server: accept, `SO_PEERCRED` check, read `CommandEnvelope`, re-verify signature against the pinned pubkey, dispatch capability.
- [ ] 2.4 Implement capabilities via `systemctl` (or `zbus`): `reset_failed`, `restart_unit`, read-only `unit_state`. Return `ActionResult` with the observed post-state.
- [ ] 2.5 Integration test against a real transient unit in a container/VM (gated to keep `nix flake check` hermetic); verify tests pass.

### 3. Signed command channel (#114)

- [ ] 3.1 Write tests: server signs an envelope the agent verifies; agent rejects forged/expired/replayed envelopes; idempotency ledger skips a re-delivered `command_id`.
- [ ] 3.2 Control plane: generate/load the Ed25519 keypair at startup; add `command_signing_pubkey` to `EnrollResponse`; persist the private key `0600`.
- [ ] 3.3 Agent: pin the pubkey at enrollment (write alongside `ca.crt`); load it on startup.
- [ ] 3.4 Agent `remediation/` module: long-poll `GET /agents/{id}/commands` (mTLS), verify each envelope, dedupe via an on-disk executed-`command_id` ledger, call the actuator over the socket, `POST` the `ActionResult`.
- [ ] 3.5 Define a `CommandSource` trait so NATS request/reply can replace long-poll later without touching the executor; verify tests pass.

### 4. Control-plane remediation orchestrator (#115)

- [ ] 4.1 Write tests: a `FailedUnit` message yields a `failed-unit-restart` proposal; approval produces a signed envelope queued for the agent; the audit row captures each transition.
- [ ] 4.2 Prepare (P1-minimal): on a `FailedUnit` event, direct-match the `failed-unit-restart` template and build a `RemediationProposal` (no LLM/recall needed in P1; rationale can be a templated string — LLM phrasing is a later enhancement).
- [ ] 4.3 Approval queue: persist proposals; `POST /remediations/{id}/approve|reject` guarded by the existing OIDC auth, recording the approver identity.
- [ ] 4.4 On approve: sign a `CommandEnvelope`, enqueue for the agent's long-poll, append to the `RemediationRecord` audit table (new SQLx migration, append-only).
- [ ] 4.5 Ingest `ActionResult` from the agent → close the record; verify tests pass.

### 5. Portal: approval queue + audit timeline (#119)

- [ ] 5.1 Write component/API tests (TanStack Query hooks; render pending proposal; approve action calls the endpoint).
- [ ] 5.2 Approval queue page: list pending proposals (fault, template+params, rationale, risk tier) with approve/reject.
- [ ] 5.3 Action audit timeline: per-host/fleet view of proposal → decision → execution → result with the approver + signature reference.
- [ ] 5.4 Wire to the #115 endpoints; verify tests pass and the build is clean.

### 6. NixOS module (#120)

- [ ] 6.1 Add a hardened `ravn-actuator` systemd unit to `nixos/modules/agent.nix` (the only privileged unit: `ProtectSystem=strict`, `PrivateTmp`, `NoNewPrivileges` where the capability set allows, socket `0660` owned by the ravn group).
- [ ] 6.2 Add `services.ravn.agent.remediation` options: `enable`, control-plane URL for command polling, pinned-key path, actuator socket path.
- [ ] 6.3 Wire unprivileged `ravnd` ↔ actuator socket permissions; document the trust boundary in the module.

### 7. End-to-end + security tests (#121, partial for P1)

- [ ] 7.1 NixOS VM test: inject a failed unit → assert a proposal appears → approve via API → assert the actuator restarts it → assert `unit_state == active` → assert the audit row.
- [ ] 7.2 Security negatives: forged signature rejected; expired/replayed envelope rejected; a simulated `ravnd` cannot invoke a non-whitelisted capability.
- [ ] 7.3 Ensure the whole suite runs under `nix flake check`.

## Risks & mitigations

- **Long-poll vs. eventual NATS request/reply churn** → the `CommandSource` trait (3.5) isolates the channel so the swap is local.
- **mTLS termination for the command endpoints** → reuse the existing enrollment CA/auth path rather than introducing a parallel scheme; confirm the server already terminates client-cert auth or add it once, shared with ingest.
- **Actuator privilege creep** → P1 ships exactly three capabilities; any new one is a reviewed `ravn-actuator` change, never config.
- **Scope drift into P2–P5** → the "Out of scope" list is the guardrail; P1 is intentionally manual-approval-only with no recall/rollback.

## Tracking

Update epic #122's checklist as each issue closes. P1 is done when #112–#115, #119,
#120 are closed and #121's P1 slice (7.1–7.3) is green.
