#!/usr/bin/env bash
# End-to-end test for the Kubernetes controller (#60).
#
# Spins up the control plane (Postgres + NATS + ravn-server) via docker compose,
# creates a k3d cluster *on the same docker network* (so in-cluster pods reach
# `nats` by name — no host networking), deploys the controller from the #59
# manifests, creates a crashlooping pod, and asserts a `kube_workload`
# CrashLoopBackOff/BackOff message reaches the control-plane API and lands in
# Postgres.
#
# Self-contained and idempotent: it tears everything down on exit.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"

CLUSTER="${E2E_CLUSTER:-ravn-e2e}"
PROJECT="${E2E_PROJECT:-ravn}"
NETWORK="${PROJECT}_default"
IMAGE="${E2E_IMAGE:-ravn-k8s:e2e}"
API="${RAVN_API:-http://localhost:18090}"
TIMEOUT="${E2E_TIMEOUT:-180}"
COMPOSE="docker compose -p ${PROJECT}"

log() { echo "==> $*"; }

cleanup() {
  log "cleanup"
  k3d cluster delete "$CLUSTER" >/dev/null 2>&1 || true
  $COMPOSE down -v >/dev/null 2>&1 || true
}
trap cleanup EXIT

log "build runtime image ($IMAGE)"
docker build -t "$IMAGE" .

log "bring up control plane (postgres + nats + control-plane)"
$COMPOSE up -d --build postgres nats control-plane

log "wait for the control plane to be healthy at $API"
deadline=$((SECONDS + TIMEOUT))
until curl -sf "$API/health" >/dev/null 2>&1; do
  [ $SECONDS -ge $deadline ] && {
    echo "FAIL: control plane never became healthy"
    $COMPOSE logs --no-color | tail -100
    exit 1
  }
  sleep 2
done
log "control plane healthy"

log "create k3d cluster '$CLUSTER' on docker network '$NETWORK'"
k3d cluster create "$CLUSTER" --network "$NETWORK" --wait --timeout 150s
export KUBECONFIG
KUBECONFIG="$(k3d kubeconfig write "$CLUSTER")"

log "import controller image into the cluster"
k3d image import "$IMAGE" -c "$CLUSTER"

log "deploy namespace + RBAC (from the #59 manifests)"
kubectl apply -f deploy/k8s/00-namespace.yaml
kubectl apply -f deploy/k8s/10-rbac.yaml

log "deploy the controller (NATS transport to the in-network control plane)"
kubectl apply -f - <<YAML
apiVersion: apps/v1
kind: Deployment
metadata:
  name: ravn-controller
  namespace: ravn-system
  labels:
    app.kubernetes.io/part-of: ravn
    app.kubernetes.io/component: controller
spec:
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/component: controller
  template:
    metadata:
      labels:
        app.kubernetes.io/part-of: ravn
        app.kubernetes.io/component: controller
    spec:
      serviceAccountName: ravn-controller
      containers:
        - name: controller
          image: ${IMAGE}
          imagePullPolicy: IfNotPresent
          command: ["ravn-controller"]
          env:
            - { name: NATS_URL, value: "nats://nats:4222" }
            - { name: RAVN_CLUSTER, value: "ravn-e2e" }
            - { name: RAVN_LOG, value: "info" }
YAML

log "wait for the controller to roll out"
kubectl -n ravn-system rollout status deploy/ravn-controller --timeout=150s

log "create a crashlooping workload"
kubectl apply -f k8s/test-workloads.yaml

log "assert a kube_workload event reaches the control-plane API"
deadline=$((SECONDS + TIMEOUT))
until curl -sf "$API/api/events?limit=100" 2>/dev/null | grep -q '"source":"kube_workload"'; do
  [ $SECONDS -ge $deadline ] && {
    echo "FAIL: no kube_workload event reached the API within ${TIMEOUT}s"
    kubectl -n ravn-system logs deploy/ravn-controller --tail=80 || true
    exit 1
  }
  sleep 3
done
log "kube_workload event present in the API"

log "assert it persisted in Postgres"
count="$($COMPOSE exec -T postgres psql -U ravn -d ravn -tAc \
  "select count(*) from events where source='kube_workload'")"
count="$(echo "$count" | tr -d '[:space:]')"
log "kube_workload rows in Postgres: ${count}"
[ "${count:-0}" -ge 1 ] || { echo "FAIL: no kube_workload rows in Postgres"; exit 1; }

log "PASS: K8s controller -> control plane -> Postgres verified in a real cluster"
