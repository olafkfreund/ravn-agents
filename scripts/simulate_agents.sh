#!/usr/bin/env bash
# Periodically publishes NATS heartbeats for all 14 registered agents
# and updates their labels for clustering/topology group testing.

API="http://127.0.0.1:18080"
NATS_SERVER="nats://127.0.0.1:14222"

# Define the agents, their hosts, and labels (JSON format)
agents=(
  "019ed000-0000-7000-8000-0000000000de|demo-host|{\"kind\":\"host\",\"env\":\"demo\"}"
  "019e9700-0000-7000-8000-00000000ae01|prod-eu|{\"kind\":\"gke-cluster\",\"cluster\":\"gke-eu-prod\",\"env\":\"prod\"}"
  "019e9700-0000-7000-8000-00000000ae02|prod-us|{\"kind\":\"eks-cluster\",\"cluster\":\"eks-us-prod\",\"env\":\"prod\"}"
  "019e9700-0000-7000-8000-00000000ae03|prod-us|{\"kind\":\"eks-cluster\",\"cluster\":\"eks-us-prod\",\"env\":\"prod\"}"
  "019e9700-0000-7000-8000-00000000ae04|staging|{\"kind\":\"aks-cluster\",\"cluster\":\"aks-staging\",\"env\":\"staging\"}"
  "019ed000-2222-7000-8000-000000000019|unknown|{\"kind\":\"host\",\"env\":\"unknown\"}"
  "019e9219-6d81-70d2-9651-3a1ee7d149d2|ravn-dev|{\"kind\":\"k3d-cluster\",\"cluster\":\"ravn-dev\",\"env\":\"dev\"}"
  "019e9300-0000-7000-8000-0000000000dd|ravn-dev|{\"kind\":\"k3d-cluster\",\"cluster\":\"ravn-dev\",\"env\":\"dev\"}"
  "019e9500-0000-7000-8000-000000000a02|ravn-dev|{\"kind\":\"k3d-cluster\",\"cluster\":\"ravn-dev\",\"env\":\"dev\"}"
  "019e9600-0000-7000-8000-000000000b02|ravn-dev|{\"kind\":\"k3d-cluster\",\"cluster\":\"ravn-dev\",\"env\":\"dev\"}"
  "019e9400-0000-7000-8000-0000000000ff|ravn-dev|{\"kind\":\"k3d-cluster\",\"cluster\":\"ravn-dev\",\"env\":\"dev\"}"
  "019e91bd-821a-7fe0-8d2d-f4063cd4a64b|ravn-dev|{\"kind\":\"k3d-cluster\",\"cluster\":\"ravn-dev\",\"env\":\"dev\"}"
  "019e91de-4985-7740-994a-6cc5b8a7c1e3|ravn-dev|{\"kind\":\"k3d-cluster\",\"cluster\":\"ravn-dev\",\"env\":\"dev\"}"
  "019e9200-0000-7000-8000-0000000000bb|ravn-dev|{\"kind\":\"k3d-cluster\",\"cluster\":\"ravn-dev\",\"env\":\"dev\"}"
)

echo "Setting up labels for agents..."
for item in "${agents[@]}"; do
  IFS='|' read -r id host labels <<< "$item"
  echo "Setting labels for agent $host ($id)..."
  curl -s -X PUT "$API/api/agents/$id/labels" \
    -H "Content-Type: application/json" \
    -d "$labels" > /dev/null
done

echo "Starting NATS heartbeat loop..."
uptime=100
while true; do
  sent_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
  for item in "${agents[@]}"; do
    IFS='|' read -r id host labels <<< "$item"
    payload="{\"agent_id\":\"$id\",\"host\":\"$host\",\"sent_at\":\"$sent_at\",\"uptime_secs\":$uptime,\"inference_enabled\":false,\"events_published\":0}"
    nats pub --server "$NATS_SERVER" "ravn.heartbeat.$id" "$payload" > /dev/null 2>&1
  done
  echo "Heartbeats sent at $sent_at"
  uptime=$((uptime + 20))
  sleep 20
done
