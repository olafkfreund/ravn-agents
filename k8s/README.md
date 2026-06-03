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

The `crasher` events are exactly what the controller (#55) will watch via the
Events API, mapping each `reason` to a severity with
`ravn_core::kube_severity_for_reason`.
