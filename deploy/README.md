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

## Portal user authentication (OIDC + RBAC) (#26)

Human access to the portal/API can be gated by **OpenID Connect** with
viewer/admin roles. The control plane validates the user's OIDC access token
(Bearer) against the IdP's JWKS — RS256 signature, issuer, audience, expiry —
statelessly, and maps a groups claim to a role. The static
`RAVN_ADMIN_TOKEN`/`RAVN_VIEWER_TOKEN` remain as a dev/bootstrap fallback.

Server env:

| Var | Purpose | Default |
| --- | --- | --- |
| `RAVN_OIDC_ISSUER` | IdP issuer URL (enables user auth) | — |
| `RAVN_OIDC_JWKS_URL` / `_FILE` | IdP JWKS source | — |
| `RAVN_OIDC_AUDIENCE` | Expected token audience (the OIDC client id) | — |
| `RAVN_OIDC_CLIENT_ID` | Public client id the SPA uses (defaults to audience) | — |
| `RAVN_OIDC_GROUPS_CLAIM` | Claim holding group memberships | `groups` |
| `RAVN_OIDC_ADMIN_GROUP` | Group granting admin (mutating API) | — |
| `RAVN_OIDC_VIEWER_GROUP` | If set, group required for any access | — (any authed = viewer) |
| `RAVN_OIDC_SCOPES` | Scopes the SPA requests | `openid profile email groups` |

RBAC: safe methods (GET) need viewer; mutating methods (PUT/DELETE) need admin.
The portal discovers config from the public `GET /auth/config`, performs the
**authorization-code + PKCE** flow against the IdP, and presents the access
token as a bearer; `GET /api/me` returns the caller's role for role-aware UI
(e.g. viewers see labels read-only).

Notes / follow-ups: tokens are validated as **RS256** (the common OIDC default);
the IdP must issue access tokens whose audience matches `RAVN_OIDC_AUDIENCE`.
The live WebSocket feed (`/ws/events`) does not yet carry the bearer, so under
user auth the portal falls back to polling for updates — a tracked follow-up.
