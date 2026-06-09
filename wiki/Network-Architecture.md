# Network Architecture & Port Allocations

This document maps out the network ports and transport protocols used within the Ravn SRE control plane and agent daemon networks.

---

## 1. Port Mapping Matrix

| Service | Port | Protocol | Description |
| :--- | :--- | :--- | :--- |
| `ravn-nats` | `14222` | TCP | Control plane messaging bus (agents telemetry & actuator commands) |
| `ravn-server` | `18080` | TCP | REST API backend and Portal dashboard server |
| `portal-dev` | `5318` | TCP | React web portal frontend |
| `k8s-apiserver` | `16443` | TCP | K3d local cluster API access |
| `cosmic-connect` | `1716` | UDP/TCP | COSMIC desktop network daemon listener |

---

## 2. Common Network Failures

### Scenario A: NATS Port `14222` Already In Use
If NATS fails to start, verify if another instance (or a local developer container) is already bound to port `14222`.

#### Diagnostics
Check process binding:
```bash
sudo lsof -i :14222
```

#### Remediation
Kill the stale process or stop the active container:
```bash
docker stop $(docker ps -q --filter publish=14222)
```

---

### Scenario B: Device Discovery Port Conflict (`1716`)
In development networks utilizing Waydroid or mobile device links, the COSMIC Connect desktop daemon or KDE Connect daemon binds to port `1716`. Port conflicts on `1716` will prevent discovery packets from reaching SRE listeners.

#### Symptoms
- Waydroid or local SRE agents fail to auto-discover local test daemons.
- Logs report: `failed to bind UDP socket on port 1716: address already in use`.

#### Remediation
Verify what service is listening:
```bash
ss -tulpn | grep 1716
```
If `kdeconnectd` is active, stop it to free up the port:
```bash
killall kdeconnectd
```
Re-run the simulator to verify connection status:
```bash
nix develop --command devenv shell ./scripts/simulate_agents.sh
```
