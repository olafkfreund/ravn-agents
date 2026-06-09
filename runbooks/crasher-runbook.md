# Crasher Service Runbook

* **Service:** `crasher`
* **Owner Team:** `team-platform`
* **On-Call Contact:** `@platform-oncall`
* **Severity Tier:** Tier-2 (Non-production testing)

## Overview
The `crasher` pod is a mock workload deployed in the `ravn-test` namespace. It is designed to crash continuously to simulate pod failures, producing event loops, logging traces, and testing automated self-healing remediation limits.

---

## Failure Scenario: Persistent CrashLoopBackOff

### Symptoms
- Pod state is in `CrashLoopBackOff`.
- Log output shows:
  ```log
  starting
  crashing
  ```
- Exit code is `1`.

### Diagnostics
1. **Confirm Pod Name and Namespace:**
   Check the pod status:
   ```bash
   kubectl get pod crasher -n ravn-test
   ```
2. **Review Exit Code:**
   Describe the pod to verify the last state exit code:
   ```bash
   kubectl get pod crasher -n ravn-test -o jsonpath='{.status.containerStatuses[0].lastState.terminated.exitCode}'
   ```

### Remediation
> [!IMPORTANT]
> This crash loop is intentional for platform simulation and testing purposes. Do not scale down, delete, or alarm on this pod. 

If this pod must be temporarily stopped to clear alerts:
1. **Scale down or delete the pod (it will not be recreated since it is a raw Pod, not part of a Deployment):**
   ```bash
   kubectl delete pod crasher -n ravn-test
   ```
2. **Re-apply the workload configuration when testing needs to resume:**
   ```bash
   kubectl apply -f k8s/test-workloads.yaml
   ```
