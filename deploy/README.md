# Deploying Ravn to Kubernetes (#59)

Manifests and a Helm chart for the in-cluster agents — the workload
**controller** (#55, a Deployment) and the **node-agent** (#56, a DaemonSet).
Both are detection-only with read-only RBAC; they publish to the control plane
over the authenticated HTTP ingest endpoint (#57) using an audience-bound
projected ServiceAccount token.

The control plane itself runs **outside** the constrained cluster by default
(design decision D2) — these manifests deploy only the agents.

## Image

Both binaries ship in one image (`ravn-k8s`), built reproducibly with Nix:

```sh
nix build .#ravn-k8s-image
docker load < result            # or: result is a tarball path
# For k3d, import it into the cluster:
k3d image import ravn-k8s:latest -c ravn-dev
```

The DaemonSet overrides the entrypoint to `ravn-node-agent`; the Deployment
uses the default `ravn-controller`.

## Raw manifests

```sh
kubectl apply -f deploy/k8s/
```

Edit `RAVN_INGEST_URL` in `20-controller.yaml` / `30-node-agent.yaml` to point
at your control plane's `/ingest` endpoint, and make sure the projected token
audience (`ravn`) matches the server's `RAVN_INGEST_AUDIENCE`.

## Helm

```sh
helm install ravn deploy/helm/ravn \
  --namespace ravn-system --create-namespace \
  --set controlPlane.ingestUrl=https://ravn-control-plane.example.com/ingest \
  --set cluster=prod-eu
```

Key values (`deploy/helm/ravn/values.yaml`):

| Value | Purpose | Default |
| --- | --- | --- |
| `image.repository` / `image.tag` | Agent image | `ravn-k8s` / `latest` |
| `controlPlane.ingestUrl` | Authenticated `/ingest` endpoint | example placeholder |
| `controlPlane.tokenAudience` | Projected-token audience (matches server) | `ravn` |
| `cluster` | Cluster identity recorded on events | `default` |
| `controller.enabled` / `nodeAgent.enabled` | Toggle each agent | `true` |

## Security posture

- **Read-only RBAC** — `get/list/watch` only, on `events`/`pods` (controller)
  and `nodes` (node-agent). No write verbs, no cluster-admin.
- **Tight securityContext** — `runAsNonRoot`, `readOnlyRootFilesystem`,
  `allowPrivilegeEscalation: false`, all capabilities dropped, seccomp
  `RuntimeDefault`.
- **No hostPath / no privileged access** by default. The optional node-OS
  journald / container-stdout taps from #56 would add a tightly-scoped
  read-only hostPath here; they are intentionally left out of the default
  manifests.

## Validation

`helm lint`, `helm template … | kubectl apply --dry-run=client`, and
`kubectl apply --dry-run=client -f deploy/k8s/` all pass. End-to-end deploy on
kind/k3d is covered by #60.
