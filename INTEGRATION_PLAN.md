# M6 Hardening — Integration / Merge Plan

15 child issues of epic #158 were implemented on isolated `fix/issue-N-*` branches off `main`.
This plan sequences them into `main` with the fewest manual conflict resolutions.

> **Status:** all 15 branches committed (`fix/issue-153-airgapped` @ `eaf816c`).
> **Nothing is pushed.** All branches are local. Build + test after **every** step before continuing.

---

## 0. Conflict matrix (file → branches that touch it)

| File | Branches | Severity |
|------|----------|----------|
| `crates/ravn-actuator/src/lib.rs` | **#144, #145, #146, #150** + WIP | 🔴 highest — 3 structural refactors (async, validation, keyring) + executor routing |
| `crates/ravn-server/src/remediation.rs` | **#143, #145, #149, #150, #151** + WIP | 🔴 highest — 5 branches |
| `crates/ravn-core/src/remediation.rs` | **#145, #146, #150** + WIP | 🟠 high — #145 & #146 both add `Capability` variants |
| `crates/ravn-server/src/api.rs` | #143, #149, #150 | 🟠 |
| `crates/ravn-server/src/main.rs` | #143, #149, #150, **#153** | 🟠 |
| `crates/ravn-server/src/config.rs` | #150, **#153** | 🟡 |
| `crates/ravn-server/src/db.rs` | #143, #149 + WIP | 🟡 |
| `crates/ravn-k8s/src/lib.rs` | #146, #147 + WIP | 🟡 (module declarations — append-only, easy) |
| `crates/ravn-k8s/Cargo.toml` | #146, #147 | 🟡 |
| `crates/ravn-agent/src/remediation.rs` | #148, #150 | 🟡 |
| `crates/ravn-core/src/lib.rs` | #145, #149 | 🟢 (re-export lines) |
| `flake.nix` | #156, #157 (+#153) | 🟢 (separate package/check entries) |
| `Cargo.toml` (root) | #156 | 🟢 |
| `Cargo.lock` | #146,#147,#148,#150,#151,#156,#157 | ⚪ ignore — regenerate with `cargo build` |
| `portal/src/components/Layout.tsx` | #149 + WIP | 🟡 |
| `docs-techdocs/*`, `docs/*` | #152, #155, #156 | 🟢 (disjoint files) |

**Single-branch files (no conflict):** everything in #154 (`dist/`, `.github/`), #155 (`deploy/helm/`), #156 (`crates/ravn-mcp/`), #157 (`crates/ravn-eval/`), plus #149's `metrics.rs`/`policy.rs`/`ingest.rs`/`SystemHealthBanner.tsx`, #150's `ravn-crypto`/`command.rs`/`config.rs`, etc.

---

## 1. Uncommitted WIP — resolve FIRST (blocks all merges)

The working tree has changes that **collide by path** with the branches. Git cannot merge #146/#147 while these untracked files exist.

### WIP-only (NOT in any branch — must be preserved)
- `crates/ravn-k8s/src/bin/ravn-controller.rs` (M)
- `crates/ravn-k8s/src/watcher.rs` (M)
- `crates/ravn-actuator/Cargo.toml` (M)
- `k8s/test-workloads.yaml` (M)
- `portal/`: `App.tsx`, `AgentDrawer.tsx`, `Sidebar.tsx`, `topology/AgentNode.tsx`, `topology/GroupNode.tsx`, `index.css`, `Agents.tsx`, `Remediations.tsx`, `Topology.tsx`, `vite.config.ts`, `openapi.json`, `api/schema.d.ts` (M)
- `policy/dev.toml`, `demo/command-ledger`, `scripts/simulate_agents.sh` (untracked)

### WIP superseded by a tested branch (likely discard — DIFF to confirm)
- `crates/ravn-k8s/src/logs.rs` (untracked) → #147 (bounded TTL/LRU version)
- `templates/k8s-pod-restart.toml`, `templates/k8s-pod-log-restart.toml` (untracked) → #146
- overlapping edits in `ravn-core/src/remediation.rs`, `ravn-actuator/src/lib.rs`, `ravn-server/src/remediation.rs`, `db.rs`, `ravn-k8s/src/lib.rs`, `portal/Layout.tsx`

### WIP to delete (per #156)
- `scripts/ravn-mcp.js`, `scripts/ravn_mcp_server.py`

**Recommended action:**
```bash
git switch -c wip/pre-integration         # shelve EVERYTHING for reference
git add -A && git commit -m "wip: pre-integration snapshot"
git switch main
git switch -c integration/m6-hardening     # clean integration branch off main
```
Then, for each overlapping file, `git diff wip/pre-integration -- <file>` to decide whether the WIP carried anything the agent branch missed. Re-apply the WIP-only files (controller, watcher, portal, policy/dev.toml, etc.) onto the integration branch after the code merges land.

---

## 2. Merge order

Merge onto `integration/m6-hardening`. **`cargo build --workspace && cargo test --workspace` after each.** `git checkout --theirs Cargo.lock` (or just `cargo build` to regenerate) on every lockfile conflict.

### Wave 1 — Leaf branches, zero/low conflict (bank the easy wins)
1. **#148** agent-backoff — `ravn-agent` only
2. **#157** ravn-eval — new crate + `flake.nix`
3. **#154** host-install — `dist/`, `.github/`, docs
4. **#155** helm — `deploy/helm/`, docs (executor stays gated)
5. **#156** mcp-server — new crate + `flake.nix` + root `Cargo.toml`; **also `git rm scripts/ravn-mcp.js scripts/ravn_mcp_server.py`**

> `flake.nix` conflicts among #157/#156 are additive (different `packages`/`checks` keys) — accept both hunks.

### Wave 2 — Actuator structural stack (STRICT ORDER — dependency chain)
6. **#144** async-actuator — changes the `CapabilityExecutor::run` signature to return a `Future`. Land the structural refactor **first**.
7. **#145** k8s-id-validation — adds validators + `Capability` variants in `ravn-core`; validation calls in actuator. Re-apply its actuator hunks on top of #144's async signature.
8. **#146** k8s-executor — **depends on #144 + #145.** Resolve the `ravn-core/src/remediation.rs` `Capability`-variant overlap with #145 here (union the variants; dedup any both added). Remove #146's `Handle::current().block_on` bridge now that #144's async trait is present. `ravn-k8s/src/lib.rs` module lines union with #147's.

### Wave 3 — Server stack (ascending size, so big ones resolve last)
9. **#143** postgres-audit — `remediation.rs` (`RemediationStore`→Postgres), `db.rs`, `api.rs`, `main.rs`, migration `0005`.
10. **#151** template-conditions — `remediation.rs` `match_event`/`load_dir`/`resolve_params`. Mostly disjoint from #143's `RemediationStore`; reconcile within `remediation.rs`.
11. **#149** observability — server-wide instrumentation + `event.rs`/`payload.rs`/`metrics.rs` + portal. Resolve `remediation.rs`/`api.rs`/`main.rs`/`db.rs`/`Layout.tsx` against the now-merged tree.
12. **#150** key-rotation — **largest cross-cutting branch (16 files).** Merge near-last so it resolves once against everything: `ravn-core` (remediation+enrollment), `ravn-crypto`, actuator (lib+main), agent (remediation+enrollment+main), server (remediation+api+command+config+main). Verify the keyring `kid` plumbing survives the other actuator/server edits.

### Wave 3.5 — Air-gapped (server-coupled)
13. **#153** airgapped — edits `ravn-server/src/config.rs` (⇄ #150) and `src/main.rs` (⇄ #143/#149/#150), so merge **after the server wave**, not as a leaf. Its `is_airgapped()`/`load_jwks()` guard sits alongside #150's config changes — accept both. `flake.nix` additions (`nixosConfigurations`, `packages.airgapped-e2e`) union with #156/#157. New `nixos/` files + docs are conflict-free.

### Wave 4 — Docs last
14. **#152** docs-honesty — **merge LAST and UPDATE before committing.** It currently labels "K8s execution deferred → #146" and treats `ravn-eval` as outstanding. After #146/#157/#153 land, those statements are stale — flip the roadmap to reflect that K8s execution, the eval harness, **and the air-gapped profile** are now in-tree. Resolves the only self-contradiction in the set.

---

## 3. Per-hotspot reconciliation cheatsheet

- **`ravn-actuator/src/lib.rs`** (#144→#145→#146→#150): async trait is the base; validation is a guard inside `handle_command`; executor routing is a rejection branch; keyring is a parameter threaded through `serve`/`handle_command`. All four are *additive at different sites* — conflicts are mechanical once #144's signature is the baseline. Build after each.
- **`ravn-core/src/remediation.rs`** (#145 vs #146 `Capability` variants): union the enum variants; ensure `is_read_only`, `resolve`, `placeholders_in_capability`, and any exhaustive `match` cover the **combined** set (each agent only added arms for its own variants).
- **`ravn-server/src/remediation.rs`** (#143/#145/#149/#150/#151): #143 owns `RemediationStore` (storage), #151 owns `match_event`/`resolve_params` (matching), #145 owns param validation, #149 adds metrics/event emission in `prepare`, #150 sets `kid` in `build_command`. Distinct functions → resolve by accepting each at its own site.

---

## 4. Verification gates (before merging `integration/m6-hardening` → `main`)

- `cargo build --workspace` + `cargo test --workspace` green
- `cargo clippy --workspace --all-targets -- --deny warnings`
- `cargo fmt --check`
- `nix flake check` (covers #156/#157/#153 additions)
- Portal: `cd portal && npm ci && npm run build` (validates #149 + #152 + WIP portal changes together)
- The **environment-gated** tests each branch flagged, run where you have the env:
  - #143 `cargo test -p ravn-server -- --ignored audit_trail_survives_restart` (needs Postgres)
  - #146 k3d E2E heal loop (needs cluster + NATS + signer)
  - #150 rotation tests already pass in-tree; re-run in the VM test
  - #154 fresh Ubuntu 24.04 quickstart walk-through
  - #155 real `k3d` + `helm install` → events in portal
  - #153 air-gapped VM test with outbound networking blocked

---

## 5. Suggested PR strategy

Either one stacked PR per wave (4 PRs — reviewable, preserves order), or one `integration/m6-hardening` → `main` PR with `Closes #143 #144 … #157` in the body (the issues are already closed, so use plain references). Given the hotspot overlap, **stacked-by-wave** is safer to review.
