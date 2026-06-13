# SRE Runbook Authoring Guide & Standards

This guide outlines the standardized structure, formatting, and content requirements for SRE runbooks in Ravn. Following these guidelines ensures that both human operators and automated Ravn agents (via the Unified Knowledge MCP Server) can successfully parse, search, and execute diagnostic and remediation steps.

---

## 1. Runbook File Format & Structure

All runbooks must be written in **GitHub Flavored Markdown (`.md`)** and placed in the designated local runbooks directory (configured in the Portal as `local_runbooks_dir`, typically `runbooks/` or `/wiki/`).

### Standard Markdown Outline
A standard runbook should follow this hierarchical structure:

1. **Title (`# H1`)**: The name of the service or system component.
2. **Metadata Section**: Owner team, on-call handlers, and service scope.
3. **Overview**: Brief description of the service architecture and dependencies.
4. **Failure Scenarios (`## H2` or `### H3`)**: Distinct sections for each known failure mode containing:
   - **Symptoms**: Logs, error messages, or metrics to look for.
   - **Diagnostics**: Commands to inspect and verify the issue.
   - **Remediation**: Step-by-step commands to resolve the issue.

---

## 2. Required Content & Formatting

To allow agents to programmatically parse and suggest remediations, follow these conventions:

### A. Title and Metadata
Every runbook must start with an H1 title and a bulleted metadata list:
```markdown
# Service Name Runbook

* **Service:** `service-identifier`
* **Owner Team:** `team-name`
* **On-Call Contact:** `@slack-handle` or `team-email@company.com`
* **Severity Tier:** `Tier-1` (Critical) / `Tier-2` (Standard)
```

### B. Standardized Failure Scenarios
For every failure mode, clearly define the three phases:

* **Symptoms**:
  - List exact log strings or error patterns. For example: `[ERROR] Connection refused on port 14222`.
  - Specify visual alerts or status changes.
* **Diagnostics**:
  - Provide safe, read-only commands first (e.g., status checks, logs query).
  - Wrap all commands in markdown code blocks with syntax highlighting (e.g. ` ```bash `).
* **Remediation**:
  - Outline the precise recovery steps.
  - Separate automated/safe actions (e.g. restarting a service, rolling back a deployment) from dangerous ones requiring manual confirmation.
  - Document all commands clearly inside code blocks.

---

## 3. Real-Life Scenario: NATS Message Broker Outage

Below is an example of a real-life runbook scenario for a NATS connection outage.

```markdown
# NATS Message Broker (ravn-nats) Runbook

* **Service:** `ravn-nats`
* **Owner Team:** `team-platform`
* **On-Call Contact:** `@platform-oncall`
* **Severity Tier:** Tier-1 (Critical Infrastructure)

## Overview
The `ravn-nats` service acts as the central messaging bus for communication between `ravn-controller`, `ravn-server`, and the distributed SRE agents. If NATS is down, telemetry events will not be delivered, and agent actuator commands will fail.

---

## Failure Scenario: Agent Connection Failures (Broker Unreachable)

### Symptoms
- `ravn-agent` logs display connection retries:
  ```log
  [ERROR] ravn_agent: failed to connect to NATS server: Connection refused (os error 111)
  ```
- Ravn Portal dashboard shows all agents in a `Disconnected` or `Stale` state.
- Actuator actions time out or return `NatsError: No responders`.

### Diagnostics
1. **Check NATS Port Binding on the Host:**
   Run `netstat` or `ss` to verify if port `14222` is active and listening:
   ```bash
   ss -tuln | grep 14222
   ```
2. **Inspect K8s Pod Status (if deployed in Kubernetes):**
   Check if the NATS pods are running and healthy:
   ```bash
   kubectl get pods -n ravn-test -l app=nats
   ```
3. **View Pod Events for Resource Crashing:**
   If a pod is in `CrashLoopBackOff`, view its detailed termination logs:
   ```bash
   kubectl describe pod -n ravn-test -l app=nats
   ```

### Remediation

#### Option A: Running in Host Systemd Mode
If NATS is running as a systemd service on the host, restart the daemon:
```bash
sudo systemctl restart nats-server.service
```
Verify the daemon successfully restarted and is running:
```bash
systemctl status nats-server.service
```

#### Option B: Deployed in Kubernetes (ravn-test Namespace)
If the NATS statefulset/deployment is frozen or unresponsive, trigger a rolling restart:
```bash
kubectl rollout restart deployment nats -n ravn-test
```
Wait for the deployment rollout to complete successfully:
```bash
kubectl rollout status deployment nats -n ravn-test
```
```
