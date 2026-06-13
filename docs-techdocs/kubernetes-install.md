---
layout: page
title: Kubernetes Install
permalink: /kubernetes-install/
---

# Kubernetes Install Path

Ravn watches Kubernetes clusters with the same detection-only philosophy as
standalone hosts — no sidecars, no mutation, no cluster-admin. This page
covers everything from a fresh k3d cluster to events flowing in the portal.

## What gets deployed

| Component | Kind | Purpose |
|-----------|------|---------|
| `ravn-controller` | Deployment (1 replica) | Watches cluster-wide Events API; maps `CrashLoopBackOff`, `OOMKilled`, `ImagePullBackOff`, `FailedScheduling`, probe `Unhealthy`, evictions to typed `KubeWorkload` signals |
| `ravn-node-agent` | DaemonSet (1 pod/node) | Watches each node's `.status.conditions`; emits `KubeNode` signals on memory/disk/PID pressure and `NodeNotReady` |
| RBAC | ClusterRole + ClusterRoleBinding | Read-only (`get/list/watch`) on events, pods, namespaces (controller) and nodes (node-agent). No write verbs |
| ServiceAccounts | ServiceAccount × 2 | One per component with projected audience-bound tokens |

Both agents authenticate to the control plane with a projected Kubernetes
ServiceAccount token (audience `ravn`). The control plane validates the token
against the cluster's OIDC JWKS, with a Kubernetes TokenReview fallback for
environments where the JWKS endpoint is not reachable externally.

## Prerequisites

- Kubernetes 1.21+ (projected ServiceAccount token API stable from 1.21)
- `kubectl` configured for your target cluster
- `helm` v3.x (install via your package manager or `brew install helm`)
- A running Ravn control plane (see [Standalone install]({{ '/standalone-install/' | relative_url }}) or `docker-compose.yml`)
- The `ravn-k8s` container image accessible to the cluster

## Quick-start (k3d local cluster)

The fastest way to see the full flow is with the included k3d dev cluster.
The dev shell (`direnv allow` or `devenv shell`) provides `k3d`, `kubectl`,
`k9s`, and `helm`.

```sh
# 1. Start the dev cluster (writes a project-local kubeconfig at
#    .devenv/state/kubeconfig — never touches your ~/.kube/config).
k3d-up

# 2. Apply the test workloads (healthy pod + crasher in CrashLoopBackOff).
kubectl apply -f k8s/test-workloads.yaml

# 3. Build and import the agent image.
nix build .#ravn-k8s-image
k3d image import ravn-k8s:latest -c ravn-dev

# 4. Install the chart.
helm install ravn deploy/helm/ravn \
  --namespace ravn-system --create-namespace \
  --set image.repository=ravn-k8s \
  --set image.tag=latest \
  --set controlPlane.ingestUrl=http://host.k3d.internal:8080/ingest \
  --set cluster=ravn-dev

# 5. Confirm the pods are Running.
kubectl get pods -n ravn-system

# 6. Watch events from the crasher.
kubectl get events -n ravn-test --field-selector involvedObject.name=crasher
```

Within a few seconds the `CrashLoopBackOff` / `BackOff` events from the
crasher pod will appear in the portal at `http://localhost:5173` as `KubeWorkload`
signals, severity `Error`.

## Production install (Helm)

### 1. Add the image to your registry

```sh
# Build with Nix (reproducible, no Docker daemon required).
nix build .#ravn-k8s-image
skopeo copy oci-archive:result \
  docker://ghcr.io/YOUR_ORG/ravn-k8s:0.2.0
```

Or use Docker:

```sh
docker build -t ghcr.io/YOUR_ORG/ravn-k8s:0.2.0 -f Dockerfile .
docker push ghcr.io/YOUR_ORG/ravn-k8s:0.2.0
```

### 2. Configure values

Create a `values-prod.yaml` (do not commit secrets into this file — use
`--set` or a Sealed Secret / External Secret Operator for anything sensitive):

```yaml
# values-prod.yaml
image:
  repository: ghcr.io/YOUR_ORG/ravn-k8s
  tag: "0.2.0"

# Point at your control plane. Use an internal Service address if the
# control plane runs in the same cluster (mode: inCluster).
controlPlane:
  mode: external   # or: inCluster
  ingestUrl: "https://ravn-cp.YOUR_DOMAIN/ingest"
  tokenAudience: "ravn"   # must match RAVN_INGEST_AUDIENCE on the server

cluster: "prod-eu"
logLevel: "info"

controller:
  watchNamespace: ""   # "" = all namespaces (recommended for production)
  resources:
    requests: { cpu: 10m, memory: 32Mi }
    limits:   { cpu: 200m, memory: 128Mi }

nodeAgent:
  tolerations:
    - operator: Exists   # run on every node, including control-plane nodes
```

### 3. Install

```sh
helm install ravn deploy/helm/ravn \
  --namespace ravn-system \
  --create-namespace \
  --values values-prod.yaml
```

Upgrade later:

```sh
helm upgrade ravn deploy/helm/ravn \
  --namespace ravn-system \
  --values values-prod.yaml
```

### 4. Verify

```sh
# Agents running?
kubectl get pods -n ravn-system

# Controller logging events?
kubectl logs -n ravn-system deploy/ravn-controller --tail=50

# Node agent running on every node?
kubectl get pods -n ravn-system -l app.kubernetes.io/component=node-agent -o wide

# Simulated fault: CrashLoopBackOff appears in portal within ~10 s.
kubectl apply -f k8s/test-workloads.yaml
kubectl get events -n ravn-test
```

## Key values reference

| Value | Purpose | Default |
|-------|---------|---------|
| `image.repository` | Agent image repository | `ghcr.io/olafkfreund/ravn-k8s` |
| `image.tag` | Image tag | `0.2.0` |
| `controlPlane.mode` | `external` or `inCluster` | `external` |
| `controlPlane.ingestUrl` | Authenticated `/ingest` endpoint | *(required)* |
| `controlPlane.tokenAudience` | Projected-token audience (must match server) | `ravn` |
| `controlPlane.tokenMount` | Mount path for the projected token | `/var/run/secrets/ravn` |
| `cluster` | Cluster identity recorded on every event | `default` |
| `logLevel` | RAVN_LOG filter (`info`, `debug`, `warn`) | `info` |
| `controller.enabled` | Deploy the cluster-wide Events controller | `true` |
| `controller.watchNamespace` | Restrict the Events watch to one namespace | `""` (all) |
| `controller.replicaCount` | Controller replicas (single-leader design) | `1` |
| `nodeAgent.enabled` | Deploy the per-node DaemonSet | `true` |
| `nodeAgent.tolerations` | DaemonSet tolerations | tolerate all taints |
| `executor.enabled` | Deploy the pod-healing executor (#146) | `false` |
| `namespace.create` | Have the chart manage the Namespace object | `false` |
| `imagePullSecrets` | Image pull secrets | `[]` |

## Authentication: in-cluster vs external control plane

### External control plane (default — design D2)

The control plane runs **outside** the monitored cluster. Agents authenticate
using a projected ServiceAccount token with audience `ravn`:

```
k8s cluster                         External network
┌─────────────────────┐             ┌──────────────────┐
│ ravn-controller     │──HTTPS/443─▶│ ravn-server      │
│ (projected SA token)│             │ /ingest          │
└─────────────────────┘             └──────────────────┘
```

The server validates the token against the cluster's OIDC JWKS endpoint:

```sh
# On the ravn-server side:
RAVN_INGEST_OIDC_ISSUER=https://kubernetes.default.svc   # your cluster issuer
RAVN_INGEST_OIDC_JWKS_URL=https://CLUSTER_IP/openid/v1/jwks
RAVN_INGEST_AUDIENCE=ravn
```

If the JWKS endpoint is not reachable from the external control plane, enable
the TokenReview fallback:

```sh
RAVN_INGEST_TOKENREVIEW_URL=https://CLUSTER_API_SERVER
RAVN_INGEST_TOKENREVIEW_TOKEN=<credential with system:auth-delegator>
RAVN_INGEST_AUDIENCE=ravn
```

### In-cluster control plane

Set `controlPlane.mode: inCluster` and point `controlPlane.ingestUrl` at the
in-cluster Service address:

```yaml
controlPlane:
  mode: inCluster
  ingestUrl: "http://ravn-server.ravn-system.svc:8080/ingest"
```

The server uses `kubernetes.default.svc` as the OIDC issuer automatically in
this mode (no JWKS URL required when running in-cluster).

## Security posture

- **Read-only RBAC** — `get/list/watch` only. The controller touches `events`,
  `pods`, `namespaces`; the node-agent touches `nodes`. No write verbs, no
  cluster-admin, no `secrets` access.
- **Tight securityContext** — `runAsNonRoot`, `readOnlyRootFilesystem`,
  `allowPrivilegeEscalation: false`, all capabilities dropped, seccomp
  `RuntimeDefault`.
- **No hostPath / no privileged access** by default. The optional journald /
  container-stdout taps described in the design (#56) are intentionally absent;
  they would add a tightly-scoped read-only hostPath and are left for a future
  opt-in values flag.
- **Token rotation** — the projected ServiceAccount token has a 1-hour TTY and
  is re-read on every publish cycle, so kubelet rotation is picked up
  transparently.
- **Audience binding** — the projected token is scoped to audience `ravn`; it
  cannot be presented to any other relying party.

## Executor (supervised pod healing — future, issue #146)

The chart includes a pre-built, disabled slot for the `ravn-executor` component
that will land in issue #146. When enabled, it acts on pending remediation
proposals from the control plane by driving typed pod-healing operations
(eviction, force-delete, annotation updates) under the same Ed25519-signed
`CommandEnvelope` protocol the host actuator uses.

To activate **once #146 ships**:

1. Create the signing-key Secret:
   ```sh
   kubectl create secret generic ravn-executor-signing-key \
     --from-file=signing.key=/path/to/verifying.key \
     -n ravn-system
   ```

2. Enable in your values:
   ```yaml
   executor:
     enabled: true
     signingKeyFile: "/var/run/secrets/ravn-executor/signing.key"
   ```

3. Upgrade the release:
   ```sh
   helm upgrade ravn deploy/helm/ravn --namespace ravn-system --values values-prod.yaml
   ```

Until #146 is merged, `executor.enabled=false` is the only safe value. The
RBAC and Deployment templates are validated by `helm lint` but produce no
Kubernetes objects while disabled.

## Raw manifests (kubectl apply)

If you prefer to manage manifests without Helm:

```sh
# Edit RAVN_INGEST_URL in 20-controller.yaml and 30-node-agent.yaml first.
kubectl apply -f deploy/k8s/
```

The raw manifests in `deploy/k8s/` are always kept in sync with the Helm chart
and validated by `kubectl apply --dry-run=client` in CI.

## GitOps (ArgoCD / Flux)

To include the Namespace object in the Argo Application (so it is tracked in
git alongside the rest of the release):

```yaml
# values-gitops.yaml
namespace:
  create: true
```

Point ArgoCD at `deploy/helm/ravn` with `targetRevision: HEAD` and
`valueFiles: [values-prod.yaml, values-gitops.yaml]`. Set
`syncPolicy.syncOptions: [CreateNamespace=false]` since the chart manages it.

## Troubleshooting

**Agents not appearing in the portal:**
- Check `kubectl logs -n ravn-system deploy/ravn-controller` for `401` errors
  (OIDC misconfiguration) or connection errors (wrong `ingestUrl`).
- Confirm the token audience matches: `kubectl create token ravn-controller -n ravn-system --audience=ravn` should return a valid JWT.

**No events from the crasher workload:**
- Confirm the controller is running: `kubectl get pods -n ravn-system`.
- The controller filters `Normal` lifecycle events; only `Warning` events and
  reasons rated `Error`+ surface. Check that the crasher is actually in
  `CrashLoopBackOff`: `kubectl get pods -n ravn-test`.

**DaemonSet pod not scheduled on control-plane nodes:**
- The default `tolerations: [{operator: Exists}]` tolerates all taints. If
  you overrode tolerations, add `effect: NoSchedule, key: node-role.kubernetes.io/control-plane`.

**TokenReview 401 from control plane:**
- Verify the credential used for `RAVN_INGEST_TOKENREVIEW_TOKEN` has
  `system:auth-delegator` RBAC. Check with:
  `kubectl auth can-i create tokenreviews --as=system:serviceaccount:ravn-system:ravn-controller`.

## See also

- [Standalone install]({{ '/standalone-install/' | relative_url }}) — NixOS module or Docker Compose for the control plane
- [Air-gapped install]({{ '/airgapped-install/' | relative_url }}) — fully offline, no external image registry
- [Architecture]({{ '/architecture/' | relative_url }}) — the three planes explained
- [`deploy/helm/ravn/values.yaml`](https://github.com/olafkfreund/ravn-agents/blob/main/deploy/helm/ravn/values.yaml) — full annotated values reference
- [`deploy/k8s/`](https://github.com/olafkfreund/ravn-agents/tree/main/deploy/k8s/) — raw Kubernetes manifests
