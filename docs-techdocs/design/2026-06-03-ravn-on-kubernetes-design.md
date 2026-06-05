# Design: Ravn on Kubernetes

> Created: 2026-06-03
> Status: Approved (design) — not yet planned/implemented
> Scope: Running Ravn detection in Kubernetes for small/low-RAM clusters

## Summary

Run Ravn inside Kubernetes to watch **both workloads (pods) and nodes**, on
clusters that are small and short on RAM, **without per-pod sidecars** and
**without any per-pod or per-node LLM**.

The design preserves Ravn's core anchor — *the LLM is never in the detection hot
path* — and extends it: deterministic detection runs cheaply **in-cluster**,
while the LLM "last mile" runs as a **shared inference service** reachable at a
configurable endpoint (default: outside the constrained cluster, co-located with
the control plane). If inference is slow or unreachable, alerts still fire with a
worse (or absent) explanation. Safe failure.

This is a deliberate departure from the host-agent design, where inference is
agent-local (`llama-server` as a systemd unit, epic #2 / #15). In Kubernetes the
in-cluster agents carry **no model**.

## Background & research

Findings that shaped the design (sources at the end):

- **Sidecar cost is per-pod; DaemonSet cost is per-node.** A monitoring sidecar
  adds ~256Mi / 250m CPU **per pod**; a DaemonSet adds ~100Mi / 100m CPU **per
  node**. On small/low-RAM pods a per-pod agent is the worst option, and a
  per-pod LLM (~1–1.5 GB for Qwen3 1.7B Q4) × N pods is a non-starter.
- **Native sidecars** (k8s ≥1.29, `initContainers` + `restartPolicy: Always`)
  exist, but their resources still count against the pod's QoS/limits.
- **Node-level observability agents are DaemonSets**, not sidecars (Datadog, OTel
  Collector, Fluent Bit, Falco). Sidecars are reserved for per-app needs (a file
  on a shared volume, a mesh proxy).
- **K8s failure signals are API events** — OOMKilled (exit 137), CrashLoopBackOff,
  FailedScheduling, evictions, probe failures — visible to **one** cluster
  watcher; no per-pod agent required.
- **LLM on K8s is a centralized service** (KubeAI, Ollama, LLMKube/llama.cpp),
  exposed over an OpenAI-compatible API — never per-pod.
- **Pods have no systemd/journald**, so Ravn's host taps (journald, failed-unit
  D-Bus, NixOS generations) don't apply inside a pod. The K8s signal surface is
  the API + container stdout; node-level taps still apply on the node OS.

## Decisions (locked in the brainstorm)

| # | Decision | Choice |
|---|----------|--------|
| D1 | Monitoring target | Both **workloads + nodes** |
| D2 | Inference location | **Configurable endpoint URL**; default external, with the control plane |
| D3 | Agent identity | **Kubernetes ServiceAccount tokens** (TokenReview/OIDC) |
| D4 | First increment | **Controller + DaemonSet together** (Phase 1) |
| D5 | Control plane / NATS | **Reuse existing**, default **outside** the workload cluster, outbound TLS |
| D6 | In-cluster agents embed an LLM? | **No** — detection-only |
| D7 | Event model | Reuse **`ravn-core`** schema; add K8s `Source`/`Payload` variants |
| D8 | Spec location | Non-published `plans/` (repo `docs/` is the Jekyll site) |

## Architecture

```
            workload cluster (small / low-RAM)
   ┌───────────────────────────────────────────────┐
   │  ravn-controller (Deployment, 1 replica)        │
   │    └─ watches K8s API (informers), read-only    │
   │  ravn-node-agent (DaemonSet, 1 / node)          │
   │    └─ node conditions, kubelet, container stdout │
   │       (+ optional node journald, RO hostPath)   │
   └───────────────┬───────────────────────────────┘
                   │ ravn-core::Message  (outbound TLS, SA-token auth)
                   ▼
        control plane (ravn-server + NATS)   ── outside the cluster ──
                   │  persist; request explanation (async)
                   ▼
        shared inference service  (inference.endpoint URL)
                   │
                   ▼
                 portal (existing events table; K8s events appear here)
```

Both detection components are **detection-only**. They emit `ravn-core::Message`s
over the existing transport. The control plane requests an explanation from the
shared inference service **asynchronously**, off the detection path.

### Components

- **`ravn-controller`** — Deployment, 1 replica (leader-election-ready for HA).
  Watches the K8s API via informers for workload health. Read-only ClusterRole,
  no node access. Footprint ~64–128Mi.
- **`ravn-node-agent`** — DaemonSet, one per node. Node + container depth: node
  conditions (MemoryPressure/DiskPressure/PIDPressure), kubelet health, container
  stdout, and optionally the node OS's journald/systemd via read-only hostPath.
  Footprint ~64–128Mi. Reuses existing host taps at node scope where sensible.
- **Shared inference service** — referenced by a configurable
  `inference.endpoint` (OpenAI-compatible). Default external, with the control
  plane. An in-cluster Deployment (KubeAI / Ollama / llama.cpp-server) is
  supported by pointing the endpoint at it.
- **Control plane + NATS** — the existing `ravn-server` + NATS, reused. Default
  deployed **outside** the workload cluster (or a separate infra namespace /
  cluster). Agents connect **outbound only**.
- **Portal** — unchanged. K8s events render in the same events table.

## Detection signals → `ravn-core`

Extend the existing typed `Source`/`Payload` enums (the `extra` map absorbs
fields not modelled yet, so this is non-breaking for existing variants):

- **`KubeWorkload`** (controller): `OOMKilled` (exit 137), `CrashLoopBackOff`,
  `BackOff`, `FailedScheduling`, `FailedMount`, `Unhealthy` (probe),
  `ImagePullBackOff`, evictions, pod/Deployment status transitions. Sourced from
  the K8s Events API + object status via informers.
  - `KubeWorkloadPayload { namespace, kind, name, reason, message, count,
    involved_object, ... , extra }`
- **`KubeNode`** (DaemonSet): node conditions, kubelet errors, disk/memory
  pressure, node-level container OOM.
  - `KubeNodePayload { node, condition, ... , extra }`

Severity mapping (deterministic): OOMKilled / CrashLoopBackOff → `error` or
`critical`; FailedScheduling / ImagePullBackOff → `warning`; status transitions →
`info`/`notice`. `category_hints` can carry namespace/labels for the topology view.

**Identity mapping (resolve in child #2/#3):** `Event.host` is free-form — use the
node name (node-agent) or the controller pod/cluster name (controller). The stable
`Event.agent_id` (`Uuid`, assigned at enrollment, epic #3) needs a defined source:
one `AgentId` per node for the DaemonSet and one per controller, derived
deterministically (e.g. from the node/cluster UID via the SA identity) so a
rescheduled pod keeps the same identity in the topology view.

## Identity & security

- **Auth**: agents present **projected ServiceAccount tokens** (short-lived,
  auto-rotated, die with the pod). Validation, in preference order:
  1. **OIDC / JWKS (default)** — the control plane verifies the projected token's
     signature against the cluster's published JWKS. Zero call-back to the API
     server, so it preserves the **outbound-only** posture and fits an external /
     multi-cluster control plane.
  2. **TokenReview (fallback)** — `POST /apis/authentication.k8s.io/v1/tokenreviews`.
     Feasible from outside the cluster but requires the control plane to reach the
     API server and hold a `system:auth-delegator` credential — in slight tension
     with outbound-only, hence not the default.
  Folds into the auth epic (#26).
- **Transport caveat (must resolve first in child #4):** today's transport is
  **NATS publish-only** (`ravn.messages.<agent_id>`) with the server subscribing
  unauthenticated — there is no per-message auth yet. Presenting the SA token
  therefore requires either (a) **NATS connection-level auth** (token/JWT on
  connect), or (b) routing K8s agents through an **authenticated HTTP/WS ingest
  endpoint** the control plane terminates and validates. This is the one place
  the design leans on infrastructure that does not exist yet.
- **RBAC**:
  - controller: read-only ClusterRole — `get/list/watch` on `events`, `pods`,
    `nodes`, `deployments`, `replicasets`, `statefulsets`, `daemonsets`.
  - node-agent: node read + tightly scoped **read-only hostPath** for journald;
    drop capabilities, `runAsNonRoot` where the journal group allows, no
    privilege escalation.
- **Network**: outbound-only to NATS/control plane (NetworkPolicy-friendly); no
  inbound to agents; no cluster-admin.

## Code structure

- Reuse `ravn-core` (schema + transport contract) and the existing NATS
  transport. K8s detection via **`kube-rs` + `k8s-openapi`**.
- Two run modes — `--mode controller` (informers) and `--mode node` (node taps).
  Lean: a K8s detection module in `ravn-agent` driving both modes from one
  binary deployed as two workloads; a separate `ravn-k8s` crate is an acceptable
  alternative if K8s deps bloat the host agent build. **Decide this before child
  #2 starts** — `kube-rs`/`k8s-openapi` are heavy, and the choice sets the agent
  crate's build profile.
- `ravn-core`: add the K8s `Source`/`Payload` variants.
- Manifests under `deploy/k8s/` (raw YAML) in Phase 1 → Helm chart in Phase 3
  (folds into packaging epic #37).

## What we deliberately do NOT do

- No per-pod sidecars for general monitoring (cost × pods; complicates every
  Deployment; wrong place on small pods).
- No LLM in any agent — sidecar, DaemonSet, or controller. Inference is shared
  and remote.
- No attempt to read systemd/journald **inside app pods** (doesn't exist there).
- No inbound network to agents; no cluster-admin RBAC.

## Phasing

- **Phase 1** — controller **+** node DaemonSet detection → existing control
  plane → portal. RBAC, SA-token auth, raw YAML manifests, `ravn-core` K8s
  schema. (Inference uses whatever endpoint the control plane already has.)
- **Phase 2** — inference-endpoint wiring + default external inference;
  explanation generation for K8s events.
- **Phase 3** — Helm chart + OCI images (#37), NetworkPolicies, leader
  election/HA, docs.

## Testing strategy

- **Hermetic unit tests**: K8s Event/object → `ravn-core::Message` mapping
  (severity, source, payload), runnable under `nix flake check`.
- **Integration**: a **kind/k3d** cluster in CI (or `kube-rs` envtest for the
  controller). Deploy the controller, create a deliberately crashlooping pod,
  assert a CrashLoopBackOff `Message` reaches the control plane and lands in
  Postgres. The host-oriented NixOS VM test (#41) stays as-is for host agents.
- **Schema backward-compat**: a regression test asserting an old-schema (pre-K8s)
  `Message` still deserializes after the new variants are added — D7's
  non-breaking promise is load-bearing.

## Tracker

Create a new epic **"Ravn on Kubernetes"** with child issues:

1. `ravn-core`: K8s `Source`/`Payload` schema variants
2. `ravn-controller`: K8s API informer + workload signal detection
3. `ravn-node-agent`: DaemonSet node/container detection
4. ServiceAccount-token auth (control-plane TokenReview/OIDC) — links #26
5. Configurable inference endpoint + async explanation for K8s events
6. `deploy/k8s/` manifests → Helm chart — links #37
7. kind-based E2E test

## Open questions / future

- HA controller via leader election (Phase 3).
- Whether to surface K8s namespaces/labels as first-class categories in the
  topology view (#31–33).
- Multi-cluster fan-in to one control plane (the SA-token/OIDC model supports it;
  needs per-cluster identity scoping).

## Sources

- [Sidecar vs DaemonSet overhead — Dash0: K8s observability with the OTel Operator](https://www.dash0.com/guides/kubernetes-observability-opentelemetry-operator)
- [eBPF sidecar-less observability (DaemonSet patterns)](https://fenilsonani.com/articles/kubernetes/ebpf-kubernetes-sidecar-less-observability/)
- [Kubernetes docs: Sidecar Containers](https://kubernetes.io/docs/concepts/workloads/pods/sidecar-containers/)
- [Native sidecar restartPolicy (k8s 1.29+)](https://oneuptime.com/blog/post/2026-02-09-native-sidecar-restart-policy/view)
- [KEP-753: Sidecar Containers](https://github.com/kubernetes/enhancements/blob/master/keps/sig-node/753-sidecar-containers/README.md)
- [KubeAI — inference operator for Kubernetes](https://www.kubeai.org/)
- [LLMKube — llama.cpp Kubernetes operator](https://github.com/defilantech/llmkube)
- [Ollama on Kubernetes (Lambda docs)](https://docs.lambda.ai/education/large-language-models/k8s-ollama-llama-3-2/)
- [Debugging CrashLoopBackOff / OOMKilled](https://oneuptime.com/blog/post/2026-01-06-kubernetes-debug-crashloopbackoff-oomkilled/view)
- [GKE: Troubleshoot OOM events](https://docs.cloud.google.com/kubernetes-engine/docs/troubleshooting/oom-events)
