#!/usr/bin/env bash
# One-shot bring-up for the Ravn self-healing demo (see demo/README.md).
#
# It stands up, on a single shared docker network:
#   * the control plane (Postgres + NATS + ravn-server) WITH the demo overlay
#     (remediation templates, a fixed command-signing key, Ollama explanations);
#   * a systemd "NixOS-style host" container running ravnd + the privileged
#     actuator + a `flaky` unit you can kill to trigger a real remediation;
#   * a k3d cluster running the Ravn controller + node-agent (DaemonSet) plus a
#     spread of healthy/failing demo workloads, all reporting over NATS;
#   * topology labels so the portal shows the cluster (☸) and the host (❄) as
#     two distinct groups.
#
# Idempotent: safe to re-run. The #1 footgun this avoids is recreating the
# control plane WITHOUT the demo overlay (which silently disables remediation) —
# always bring the stack up through this script, never a bare `docker compose up`.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"
export PATH="$HOME/.local/bin:$PATH"

CLUSTER="${RAVN_DEMO_CLUSTER:-ravn-demo}"
PROJECT="${RAVN_DEMO_PROJECT:-ravn-agents}"
NETWORK="${PROJECT}_default"
K8S_IMAGE="${RAVN_DEMO_K8S_IMAGE:-ravn-k8s:demo}"
API="${RAVN_API:-http://127.0.0.1:18090}"
COMPOSE="docker compose -p ${PROJECT} -f docker-compose.yml -f demo/docker-compose.demo.yml"
TIMEOUT="${RAVN_DEMO_TIMEOUT:-180}"
DEMO_MODEL="${RAVN_DEMO_MODEL:-qwen3:1.7b}"

log() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }

# GPU for the Ollama sidecar. Auto-detect AMD ROCm (/dev/kfd) and NVIDIA
# (nvidia-smi); override with RAVN_DEMO_GPU=rocm|nvidia|cpu. See demo/README.md
# for other GPUs/runtimes (Vulkan, external Ollama).
GPU="${RAVN_DEMO_GPU:-auto}"
if [ "$GPU" = "auto" ]; then
  if [ -e /dev/kfd ]; then GPU=rocm
  elif command -v nvidia-smi >/dev/null 2>&1; then GPU=nvidia
  else GPU=cpu; fi
fi
case "$GPU" in
  rocm)   COMPOSE="$COMPOSE -f demo/docker-compose.gpu-rocm.yml";   log "GPU: AMD ROCm (override with RAVN_DEMO_GPU)";;
  nvidia) COMPOSE="$COMPOSE -f demo/docker-compose.gpu-nvidia.yml"; log "GPU: NVIDIA CUDA (override with RAVN_DEMO_GPU)";;
  cpu)    log "GPU: none — Ollama runs on CPU (set RAVN_DEMO_GPU=rocm|nvidia to use a GPU)";;
  *)      echo "unknown RAVN_DEMO_GPU=$GPU (want rocm|nvidia|cpu|auto)" >&2; exit 1;;
esac

require() { command -v "$1" >/dev/null 2>&1 || { echo "missing dependency: $1" >&2; exit 1; }; }
require docker
require k3d
require kubectl

log "build + start the control plane and the systemd host (with demo overlay)"
$COMPOSE up -d --build

log "wait for the control plane at $API"
deadline=$((SECONDS + TIMEOUT))
until curl -sf "$API/health" >/dev/null 2>&1; do
  [ $SECONDS -ge $deadline ] && { echo "FAIL: control plane never became healthy"; $COMPOSE logs --no-color | tail -80; exit 1; }
  sleep 2
done
log "control plane healthy"

log "ensure the Ollama sidecar has the explanation model ($DEMO_MODEL)"
if $COMPOSE exec -T ollama ollama list 2>/dev/null | grep -q "${DEMO_MODEL%%:*}"; then
  log "model already present"
else
  log "pulling $DEMO_MODEL (one-time, into the ollama-demo volume)…"
  $COMPOSE exec -T ollama ollama pull "$DEMO_MODEL"
fi
# Warm the model so the first real event's explanation isn't a cold load.
$COMPOSE exec -T ollama ollama run "$DEMO_MODEL" "ok" >/dev/null 2>&1 || true

log "build the in-cluster image ($K8S_IMAGE)"
docker build -t "$K8S_IMAGE" .

if k3d cluster list 2>/dev/null | grep -q "^${CLUSTER}[[:space:]]"; then
  log "k3d cluster '$CLUSTER' already exists"
else
  log "create k3d cluster '$CLUSTER' on docker network '$NETWORK'"
  k3d cluster create "$CLUSTER" --network "$NETWORK" --wait --timeout 150s
fi

log "import $K8S_IMAGE into the cluster"
k3d image import "$K8S_IMAGE" -c "$CLUSTER"

KUBECONFIG="$(k3d kubeconfig write "$CLUSTER")"
export KUBECONFIG

log "deploy the Ravn controller + node-agent (NATS transport) and demo workloads"
kubectl apply -f demo/k8s/ravn-agents.yaml
kubectl apply -f demo/k8s/workloads.yaml
kubectl -n ravn-system rollout status deploy/ravn-controller --timeout=150s

log "label agents so topology groups them (kind: nixos vs k3d-cluster)"
deadline=$((SECONDS + 60))
until curl -sf "$API/api/agents" 2>/dev/null | grep -q 'k3d-demo'; do
  [ $SECONDS -ge $deadline ] && { echo "WARN: cluster agent not registered yet; labels may be incomplete"; break; }
  sleep 2
done
python3 - "$API" <<'PY'
import json, sys, urllib.request
API = sys.argv[1]
agents = json.load(urllib.request.urlopen(API + "/api/agents"))
for a in agents:
    host = a["host"]
    if "nixos" in host:        labels = {"kind": "nixos", "env": "demo"}
    elif host == "k3d-demo":   labels = {"kind": "k3d-cluster", "env": "demo"}
    else:                      labels = {"kind": "host", "env": "demo"}
    req = urllib.request.Request(API + f"/api/agents/{a['agent_id']}/labels",
        data=json.dumps(labels).encode(), method="PUT",
        headers={"content-type": "application/json"})
    print(f"  {host} -> {labels}  http={urllib.request.urlopen(req).getcode()}")
PY

log "DEMO READY"
cat <<EOF

  Portal (built image) : http://localhost:8088/topology
  Portal (dev server)  : cd portal && npm run dev   # http://localhost:5318/topology
  Control plane API    : $API

  Topology shows two groups: ☸ k3d-cluster (k3d-demo) and ❄ nixos (demo host).
  Kubernetes pod failures stream into Events with async LLM explanations.

  Trigger a self-healing remediation:
    scripts/demo-remediate.sh        # kills flaky.service, prints the proposal id
    # then approve in the portal's Remediations page, or:
    curl -X POST $API/api/remediations/<id>/approve \\
      -H 'content-type: application/json' -d '{"approver":"demo"}'

  Tear down:
    k3d cluster delete $CLUSTER
    $COMPOSE down -v
EOF
