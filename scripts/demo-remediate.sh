#!/usr/bin/env bash
# Trigger one self-healing remediation in the running demo: fail `flaky.service`
# on the host container and wait for the control plane to propose a fix. Prints
# the proposal id so you can approve it (in the portal, or via the API).
#
# With --auto it also approves and waits for the unit to heal — a full
# hands-off end-to-end check.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"

PROJECT="${RAVN_DEMO_PROJECT:-ravn-agents}"
COMPOSE="docker compose -p ${PROJECT} -f docker-compose.yml -f demo/docker-compose.demo.yml"
API="${RAVN_API:-http://127.0.0.1:18090}"
AUTO=0
[ "${1:-}" = "--auto" ] && AUTO=1

log() { printf '\033[1;36m==> %s\033[0m\n' "$*"; }

# The failed-unit tap emits only on a *transition* into failed, so the unit must
# first be observed `active`. It polls every 5s — start it, wait past one poll,
# then kill, or no event is produced.
log "restart flaky.service and let the agent observe it active"
$COMPOSE exec -T host-agent systemctl reset-failed flaky.service >/dev/null 2>&1 || true
$COMPOSE exec -T host-agent systemctl start flaky.service
sleep 7

log "kill flaky.service (SIGKILL → unit enters 'failed')"
$COMPOSE exec -T host-agent systemctl kill -s SIGKILL flaky.service

log "waiting for a remediation proposal…"
ID=""
for _ in $(seq 1 25); do
  ID="$(curl -sf "$API/api/remediations" 2>/dev/null \
    | python3 -c 'import sys,json
r=json.load(sys.stdin)
pend=[x for x in r if x.get("decision",{}).get("decision")=="pending"]
print(pend[0]["proposal"]["id"] if pend else "")' 2>/dev/null || true)"
  [ -n "$ID" ] && break
  sleep 2
done
[ -z "$ID" ] && { echo "FAIL: no proposal appeared (is the demo overlay loaded? run scripts/demo-up.sh)"; exit 1; }
log "proposal: $ID"

if [ "$AUTO" -eq 0 ]; then
  echo "approve it in the portal, or: curl -X POST $API/api/remediations/$ID/approve -H 'content-type: application/json' -d '{\"approver\":\"demo\"}'"
  exit 0
fi

log "approving (--auto)"
curl -sf -X POST "$API/api/remediations/$ID/approve" \
  -H 'content-type: application/json' -d '{"approver":"demo"}' -o /dev/null
for _ in $(seq 1 20); do
  # `is-active` exits non-zero while the unit is still failed — don't let that
  # trip `set -e` (a bare assignment propagates the substitution's status).
  st="$($COMPOSE exec -T host-agent systemctl is-active flaky.service 2>&1 | tr -d '[:space:]' || true)"
  res="$(curl -sf "$API/api/remediations" 2>/dev/null \
    | python3 -c "import sys,json
for x in json.load(sys.stdin):
    if x.get('proposal',{}).get('id')=='$ID': print(x.get('result',{}).get('status','')); break" 2>/dev/null || true)"
  echo "  flaky=$st  remediation=${res:-pending}"
  [ "$st" = "active" ] && [ "$res" = "succeeded" ] && { log "HEALED + SUCCEEDED"; exit 0; }
  sleep 2
done
echo "FAIL: did not reach healed+succeeded in time"; exit 1
