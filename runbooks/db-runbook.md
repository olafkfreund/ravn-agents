# Database (ravn-db) Service Runbook

* **Service:** `db`
* **Owner Team:** `team-data`
* **On-Call Contact:** `@data-oncall`
* **Severity Tier:** Tier-1 (Testing Datastore)

## Overview
The `db` pod hosts a mock netcat server listening on port `8080` in the `ravn-test` namespace. It responds with simple HTTP responses to keep the frontend running. Since this database does not persist state on disk, it can be safely restarted or recreated at any time without data loss.

---

## Failure Scenario: Port 8080 Unresponsive or Frozen

### Symptoms
- `frontend` service pod fails with `Database unreachable!` error logs.
- Liveness probe checks fail for the `db` service.

### Diagnostics
1. **Verify Pod Running Status:**
   Check if the database pod is running:
   ```bash
   kubectl get pod db -n ravn-test
   ```
2. **Test Service Connection Locally:**
   Execute a simple curl/wget from a test container to check if port `8080` responds:
   ```bash
   kubectl exec -n ravn-test db -- wget -qO- http://localhost:8080
   ```

### Remediation
Since the `db` pod is stateless and run as a single pod, recreate it if it stops responding:
1. **Delete the unresponsive pod:**
   ```bash
   kubectl delete pod db -n ravn-test
   ```
2. **Re-create the workload pod:**
   ```bash
   kubectl apply -f k8s/test-workloads.yaml
   ```
3. **Verify the new pod is Running:**
   ```bash
   kubectl get pods -n ravn-test -l app=ravn-db
   ```
