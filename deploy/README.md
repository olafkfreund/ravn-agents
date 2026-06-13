# Deploying Ravn to Kubernetes

Manifests and a Helm chart for the in-cluster agents — the workload
**controller** (a Deployment) and the **node-agent** (a DaemonSet). Both are
detection-only with read-only RBAC; they publish to the control plane over the
authenticated HTTP ingest endpoint using an audience-bound projected
ServiceAccount token.

A pre-built, **disabled** slot for the **executor** component (issue #146) is
also present — flip `executor.enabled=true` once #146 ships the binary.

The control plane runs **outside** the constrained cluster by default (design
decision D2) — these manifests deploy only the in-cluster agents.

See the full end-to-end walkthrough at
[kubernetes-install](https://olafkfreund.github.io/ravn-agents/kubernetes-install/).

---

## Image

Both controller and node-agent binaries ship in one image (`ravn-k8s`), built
reproducibly with Nix:

```sh
nix build .#ravn-k8s-image
docker load < result            # or: result is a tarball path
# Import into k3d:
k3d image import ravn-k8s:latest -c ravn-dev
```

---

## Helm (recommended)

### Quick-start

```sh
helm install ravn deploy/helm/ravn \
  --namespace ravn-system --create-namespace \
  --set controlPlane.ingestUrl=https://ravn-control-plane.example.com/ingest \
  --set cluster=prod-eu
```

### Upgrade

```sh
helm upgrade ravn deploy/helm/ravn \
  --namespace ravn-system \
  --values values-prod.yaml
```

### Key values (`deploy/helm/ravn/values.yaml`)

| Value | Purpose | Default |
|-------|---------|---------|
| `image.repository` / `image.tag` | Agent image | `ghcr.io/olafkfreund/ravn-k8s` / `0.2.0` |
| `controlPlane.ingestUrl` | Authenticated `/ingest` endpoint | example placeholder |
| `controlPlane.tokenAudience` | Projected-token audience (matches server `RAVN_INGEST_AUDIENCE`) | `ravn` |
| `cluster` | Cluster identity recorded on every event | `default` |
| `controller.enabled` | Deploy the cluster-wide Events controller | `true` |
| `controller.watchNamespace` | Restrict the Events watch to one namespace | `""` (all) |
| `nodeAgent.enabled` | Deploy the per-node DaemonSet | `true` |
| `executor.enabled` | Deploy pod-healing executor (**requires #146**) | `false` |
| `namespace.create` | Have the chart manage the Namespace (GitOps) | `false` |

### Executor slot (#146)

```yaml
# Enable ONLY after #146 merges the ravn-executor binary.
executor:
  enabled: true
  signingKeyFile: "/var/run/secrets/ravn-executor/signing.key"
```

Prerequisite: create the signing-key Secret before enabling:
```sh
kubectl create secret generic ravn-executor-signing-key \
  --from-file=signing.key=/path/to/verifying.key \
  -n ravn-system
```

---

## Raw manifests (kubectl apply)

```sh
# Edit RAVN_INGEST_URL in 20-controller.yaml / 30-node-agent.yaml first.
kubectl apply -f deploy/k8s/
```

---

## Security posture

- **Read-only RBAC** — `get/list/watch` on `events`/`pods`/`namespaces`
  (controller) and `nodes` (node-agent). No write verbs, no cluster-admin.
- **Tight securityContext** — `runAsNonRoot`, `readOnlyRootFilesystem`,
  `allowPrivilegeEscalation: false`, all capabilities dropped, seccomp
  `RuntimeDefault`.
- **No hostPath / no privileged access** by default.
- **Audience-bound token** — projected with audience `ravn`, re-read per
  publish so kubelet rotation is transparent.

---

## Validation

- `helm lint` — passes (0 chart(s) failed).
- `helm template … | kubectl apply --dry-run=client` — all objects accepted.
- `kubectl apply --dry-run=client -f deploy/k8s/` — passes.
- Real k3d e2e: covered by the dev cluster in `k8s/README.md` (`k3d-up`).
