# Local Kubernetes test cluster (#53)

A [k3d](https://k3d.io) (k3s-in-Docker) cluster for developing and testing
Ravn's Kubernetes integration — the controller (#55), node agent (#56), auth
(#57), and the k3d e2e (#60).

## Prerequisites

- Docker running (k3d launches k3s inside containers).
- The dev shell (`direnv allow` or `devenv shell`), which provides `k3d`,
  `kubectl`, `k9s`, and `helm`.

## Lifecycle (devenv scripts)

```sh
k3d-up       # create the cluster (if absent) + apply the test workloads
k3d-status   # nodes + the ravn-test pods
k3d-down     # delete the cluster
```

`k3d-up` writes a **project-local** kubeconfig to `.devenv/state/kubeconfig`
(exported as `$KUBECONFIG` in the shell), so it never touches your
`~/.kube/config`.

## What you get

- **Cluster `ravn-dev`** — 1 server + 1 agent (two nodes, so the DaemonSet node
  agent has somewhere to land). The API server binds an **uncommon** host port,
  `127.0.0.1:16443`, to avoid colliding with anything on the default `6443`.
  k3s is pinned to a modern version in [`k3d/cluster.yaml`](k3d/cluster.yaml).
- **Two test workloads** in the `ravn-test` namespace
  ([`test-workloads.yaml`](test-workloads.yaml)):
  - `healthy` — a tiny pod that sleeps and stays `Running` (the baseline);
  - `crasher` — a pod that exits non-zero on a loop → `CrashLoopBackOff`,
    emitting the `BackOff` / `CrashLoopBackOff` events the controller will turn
    into `KubeWorkload` signals (modelled by #54).

## Poking at it

```sh
kubectl get pods -n ravn-test -o wide
kubectl get events -n ravn-test --field-selector involvedObject.name=crasher
k9s -n ravn-test
```

The `crasher` events are exactly what the controller (#55) watches via the
Events API, mapping each `reason` to a severity with
`ravn_core::kube_severity_for_reason`.

## ravn-controller (#55)

`crates/ravn-k8s` builds the `ravn-controller` binary: a read-only cluster
controller that watches core/v1 Events with a kube-rs informer and publishes
each workload failure (`OOMKilled`, `CrashLoopBackOff`, `BackOff`,
`FailedScheduling`, `ImagePullBackOff`, probe `Unhealthy`, evictions) to the
control plane as a `KubeWorkload` Message — on the same `ravn.messages.<id>`
NATS subject the host agent uses, so existing ingestion handles it unchanged.

It runs as its **own binary**, not a `ravnd --mode`: the host agent is a small
CPU-only daemon, while the controller pulls the heavy kube-rs client and has a
wholly different in-cluster RBAC/Deployment story (#57/#59). Both share
`ravn-core`.

Routine `Normal` lifecycle events (`Started`, `Pulled`, …) are dropped; only
`Warning` events and reasons rated `Error`+ surface. Signals are de-duplicated
on the Event UID and re-emitted only when the aggregated `count` advances, so a
flapping pod yields one signal per genuine new occurrence.

Run it against the local cluster from the dev shell (it reads `$KUBECONFIG`
and `$NATS_URL` from the shell):

```sh
RAVN_K8S_NAMESPACE=ravn-test RAVN_CLUSTER=ravn-dev cargo run -p ravn-k8s
```

| Env var | Purpose | Default |
| --- | --- | --- |
| `NATS_URL` | Control-plane NATS | `nats://127.0.0.1:4222` |
| `RAVN_CONTROLLER_ID` | Stable identity (UUID); publish subject + event `agent_id` | generated |
| `RAVN_CLUSTER` | Cluster name recorded as event `host` | `$HOSTNAME` / `kubernetes` |
| `RAVN_K8S_NAMESPACE` | Restrict the watch to one namespace | all namespaces |

## ravn-node-agent (#56)

The same `ravn-k8s` crate also builds `ravn-node-agent`: a **DaemonSet** (one
pod per node) that watches its own Node's `.status.conditions` and publishes
node-level problems as `KubeNode` Messages — memory / disk / PID pressure, and
`Ready != True` → `NodeNotReady`. Severity comes from the shared
`kube_severity_for_reason` table, so a node and a workload signal of the same
class agree.

It restricts its watch to its own node via the downward-API `NODE_NAME`, and
emits a signal only on the **transition** into an unhealthy condition (and
re-arms once it clears), so a node under sustained pressure yields one signal,
not one per watch tick.

```sh
# Watches all nodes when NODE_NAME is unset (local dev); a DaemonSet sets
# NODE_NAME to scope each pod to its own node.
RAVN_CLUSTER=ravn-dev cargo run -p ravn-k8s --bin ravn-node-agent
```

| Env var | Purpose | Default |
| --- | --- | --- |
| `NODE_NAME` | The node to watch + record as `host` (downward API in the DaemonSet) | all nodes |
| `NATS_URL` / `RAVN_CONTROLLER_ID` / `RAVN_CLUSTER` | As for the controller above | — |

The container-stdout and node-OS journald (read-only hostPath) taps named in
#56 are deferred to the manifest work (#59), where the hostPath and
tightly-scoped securityContext wiring lives.

Both binaries were verified live against this k3d cluster: the controller maps
the `crasher` pod's `BackOff` to a `KubeWorkload` signal, and the node-agent
emits `NodeNotReady` when a node's container is stopped — each flowing through
NATS → control plane → `/api/events`.

## Authenticated ingest (#57)

In-cluster agents can authenticate to the control plane with their projected
**ServiceAccount token** instead of publishing to an unauthenticated NATS
subject. Set `RAVN_INGEST_URL` and the agents publish over HTTP, presenting the
token (re-read each publish so kubelet rotation is picked up) as a bearer
credential:

```sh
RAVN_INGEST_URL=https://ravn-control-plane/ingest \
RAVN_SA_TOKEN_FILE=/var/run/secrets/ravn/token \
  ravn-controller   # or ravn-node-agent
```

A DaemonSet/Deployment mounts an **audience-bound projected token** (audience
`ravn`) at `RAVN_SA_TOKEN_FILE` (#59 wires the volume).

The control plane validates each token against the cluster's **OIDC JWKS** —
verifying the RS256 signature, issuer, audience, and expiry locally (no
per-request API-server call). Enable it on the server with:

| Server env var | Purpose | Default |
| --- | --- | --- |
| `RAVN_INGEST_OIDC_ISSUER` | Cluster OIDC issuer (enables `/ingest`) | — |
| `RAVN_INGEST_OIDC_JWKS_URL` | Fetch the JWKS over HTTPS at startup | — |
| `RAVN_INGEST_OIDC_JWKS_FILE` | …or read the JWKS from a file (mounted ConfigMap) | — |
| `RAVN_INGEST_AUDIENCE` | Required token audience | `ravn` |

Verified live against this k3d cluster: a token minted with
`kubectl create token default -n ravn-test --audience=ravn` is accepted
(`202`) and persisted, while a missing or invalid token is rejected (`401`).

> **Follow-up:** the Kubernetes `TokenReview` *fallback* named in #57 is not yet
> implemented — the OIDC/JWKS default path is. Tracked for a later pass.

## Inference for K8s events (#58)

K8s agents are detection-only — no per-pod/per-node model — so their events
arrive without an explanation. The **control plane** fills that "last mile" in
by calling a **shared, configurable, OpenAI-compatible inference endpoint**,
**asynchronously and off the ingestion path**: the event persists immediately
(the alert has fired), and an explanation is requested in the background and
attached to the row when it returns. If the endpoint is slow or unreachable the
event simply keeps its deterministic title — **safe failure**.

Enable it on the server:

| Server env var | Purpose | Default |
| --- | --- | --- |
| `RAVN_INFERENCE_ENDPOINT` | OpenAI-compatible base URL (enables explanations); `/chat/completions` is appended | — |
| `RAVN_INFERENCE_MODEL` | Model name to request | `default` |
| `RAVN_INFERENCE_API_KEY` / `_FILE` | Optional bearer key for the endpoint | — |
| `RAVN_INFERENCE_TIMEOUT_SECS` | Per-request timeout | `30` |

Only `kube_workload` / `kube_node` events without an existing explanation are
enriched (host-agent events already carry their own). Outcomes are counted in
`ravn_explanations_generated_total` / `ravn_explanation_errors_total`.

Verified live: a bare `KubeWorkload` event ingested over NATS was explained via
a mock endpoint and the explanation (text + `suggested_check` + model) appeared
on the event in `/api/events` within ~2s.
