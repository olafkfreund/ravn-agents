# Simulation & Verification Report: SRE Runbook Integration

This report documents the verification process and output of SRE self-healing workflows inside the Ravn local cluster simulation environment (`ravn-test` namespace).

---

## 1. Test Environment Setup

We deployed the mock Kubernetes workloads defined in `k8s/test-workloads.yaml` into the `ravn-test` namespace:
- **`healthy`**: Simple sleeping container (baseline running state).
- **`crasher`**: Raw pod designed to exit with code `1` every 2 seconds, triggering `CrashLoopBackOff`.
- **`db`**: Mock netcat database server listening on port `8080`.
- **`frontend`**: Core client checking connectivity against the `db` service on a loop.

### Active Pod Status Verification
```bash
$ kubectl get pods -n ravn-test
NAME       READY   STATUS             RESTARTS      AGE
crasher    0/1     CrashLoopBackOff   1 (17s ago)   35s
db         1/1     Running            0             35s
frontend   1/1     Running            0             35s
healthy    1/1     Running            0             35s
```

---

## 2. Simulated Database Outage

To trigger a real failure event, we simulated an infrastructure crash by force-deleting the stateless `db` pod:
```bash
$ kubectl delete pod db -n ravn-test --grace-period=0 --force
```

### Log Signature Capture
Within seconds, the `frontend` container failed liveness probes and printed the failure signature:
```log
$ kubectl logs frontend -n ravn-test
wget: can't connect to remote host (10.43.96.78): Connection refused
Database unreachable!
```

---

## 3. Agent Diagnostic & Runbook Retrieval

With the outage active, we executed the Antigravity SRE agent test suite. The agent automatically invoked the `unified-knowledge` MCP server to locate relevant documentation.

### Query Input
```
"Search for troubleshooting runbooks for the 'frontend' service when the database is unreachable, and tell me who to contact."
```

### Verified Agent Response Output
The agent dynamically loaded the configurations, queried the MCP server, and output the following verified action plan:

```markdown
Based on the search results from the Unified Knowledge system and the local codebase files, here are the troubleshooting runbooks and contact details for the frontend service when the database is unreachable.

### 📞 Contact Information

| Service | Owner Team | Primary On-Call Contact | Runbook Contact Handle | Documentation Source |
| :--- | :--- | :--- | :--- | :--- |
| **frontend** | team-frontend | @alice | @frontend-oncall | runbooks/frontend-runbook.md |
| **db** | team-data | @bob | @data-oncall | runbooks/db-runbook.md |

---

### 🛠️ Troubleshooting Runbook: Database Unreachable / Connection Loop Failure

When the database is unreachable, the frontend pod will enter a CrashLoopBackOff state and report Database unreachable! in its logs. 

Follow the diagnostics and remediation steps below:

#### 1. Diagnostics
1. Check if the DB service pod is running:
   ```bash
   kubectl get pods -n ravn-test -l app=ravn-db
   ```
2. Check Port Connectivity:
   Test connection from a temporary container inside the ravn-test namespace to see if the database port is reachable:
   ```bash
   kubectl run test-connection -n ravn-test --rm -i --tty --image=busybox:1.36 -- wget -qO- http://db:8080
   ```

#### 2. Remediation
1. If the DB pod is unresponsive, crashed, or down:
   Follow the Database Runbook (db-runbook.md) to delete and recreate the stateless database pod:
   ```bash
   # Delete the unresponsive database pod
   kubectl delete pod db -n ravn-test

   # Recreate database workload
   kubectl apply -f k8s/test-workloads.yaml
   ```
2. Restart the Frontend Pod:
   Once the database is verified as online and responding, clear the failure state by restarting the frontend pod:
   ```bash
   kubectl delete pod -n ravn-test -l app=ravn-frontend
   ```
3. Verify Restoration:
   Confirm that both pods are running and stable:
   ```bash
   kubectl get pods -n ravn-test
   ```
```

---

## 4. Remediation Execution & Resolution

Using the retrieved runbook procedures:
1. **Re-applied the Database Pod**:
   ```bash
   $ kubectl apply -f k8s/test-workloads.yaml
   pod/db created
   ```
2. **Re-established connection**:
   The `frontend` pod successfully reconnected on its next loop, returning to a fully healthy status.
