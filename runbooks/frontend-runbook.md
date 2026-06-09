# Frontend (ravn-frontend) Service Runbook

* **Service:** `frontend`
* **Owner Team:** `team-frontend`
* **On-Call Contact:** `@frontend-oncall`
* **Severity Tier:** Tier-1 (User Facing Portal)

## Overview
The `frontend` pod runs a continuous loop that calls the database service (`db` on port `8080`). If the connection fails, it prints `Database unreachable!` and exits with code 1.

---

## Failure Scenario: Database Unreachable / Connection Loop Failure

### Symptoms
- Pod state enters `CrashLoopBackOff` or reports restarts.
- Log output shows:
  ```log
  Database unreachable!
  ```

### Diagnostics
1. **Check if the DB service is running:**
   Verify if the backend database pod is running:
   ```bash
   kubectl get pods -n ravn-test -l app=ravn-db
   ```
2. **Check Port Connectivity:**
   Test connection from within the namespace:
   ```bash
   kubectl run test-connection -n ravn-test --rm -i --tty --image=busybox:1.36 -- wget -qO- http://db:8080
   ```

### Remediation
1. **If the DB pod is down or unresponsive:**
   Follow the [Database Runbook](db-runbook.md) to delete and recreate the `db` pod.
2. **Restart the Frontend Pod:**
   Once the database is online, restart the frontend pod to clear the failure state:
   ```bash
   kubectl delete pod frontend -n ravn-test
   ```
3. **Verify Restoration:**
   Check that both pods are running and stable:
   ```bash
   kubectl get pods -n ravn-test
   ```
