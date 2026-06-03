#!/usr/bin/env bash
# End-to-end smoke test (#41): assert the full M0 thread works —
# agent -> NATS -> control plane -> API. Run against a live stack (e.g. the
# docker-compose demo). Configure the API base with RAVN_API.
set -euo pipefail

API="${RAVN_API:-http://localhost:18090}"
TIMEOUT="${E2E_TIMEOUT:-120}"

echo "==> waiting for the control plane at $API"
deadline=$((SECONDS + TIMEOUT))
until curl -sf "$API/health" >/dev/null 2>&1; do
  [ $SECONDS -ge $deadline ] && { echo "FAIL: control plane never became healthy"; exit 1; }
  sleep 2
done
echo "    control plane is healthy"

echo "==> waiting for the demo agent to register (agent -> NATS -> control plane)"
deadline=$((SECONDS + TIMEOUT))
until curl -sf "$API/api/agents" 2>/dev/null | grep -q '"host":"demo-agent"'; do
  [ $SECONDS -ge $deadline ] && {
    echo "FAIL: demo agent did not register within ${TIMEOUT}s"
    echo "agents: $(curl -s "$API/api/agents" || true)"
    exit 1
  }
  sleep 2
done

echo "==> PASS: demo agent registered — the M0 thread is alive"
curl -s "$API/api/agents" | head -c 400
echo
