---
layout: page
title: Host Quickstart (non-NixOS)
permalink: /host-quickstart/
---

# 5-minute quickstart — standalone Linux host

This guide gets you from a fresh Debian/Ubuntu/RHEL machine to a running Ravn
agent that can detect and heal a failed systemd unit, using:

- **docker-compose** for the control plane (PostgreSQL + NATS + ravn-server)
- **static musl binary + systemd** for the host agent (`ravnd`)

No NixOS, no container runtime on the monitored host, no package manager magic.

---

## Prerequisites

| Component | Requirement |
|-----------|-------------|
| Control-plane host | Any Linux VM with Docker + Compose v2 |
| Agent host | Any Linux host with systemd ≥ 232 and kernel ≥ 5.10 |
| Architecture | x86_64 or aarch64 |
| Outbound network | Agent host must reach control-plane port 18090 |

The control plane and the agent can run on the same machine (fine for a demo)
or on separate servers (production topology).

---

## Step 1 — Start the control plane

On the **control-plane host**, fetch the compose file and start the stack:

```bash
# Download just the quickstart compose file
curl -fsSL \
  https://github.com/olafkfreund/ravn-agents/releases/latest/download/ravn-systemd-units.tar.gz \
  | tar -xz

# Or clone the repo and use the file directly:
# git clone https://github.com/olafkfreund/ravn-agents && cd ravn-agents

docker compose \
  -f dist/docker-compose/docker-compose.quickstart.yml \
  up -d

# Verify the control plane is healthy (may take ~30 s on first run):
curl -sf http://localhost:18090/healthz && echo "control plane OK"
```

The stack exposes:

| Port | Service |
|------|---------|
| `18090` | ravn-server REST API (agents + portal API) |
| `18088` | Ravn portal UI |
| `18222` | NATS monitoring (optional) |

> **Tip:** in production, put nginx/Caddy with TLS in front of port 18090 and
> block direct access. The `RAVN_BIND` variable in the compose file controls
> what address ravn-server listens on.

---

## Step 2 — Install the static agent binary

On the **agent host**, download the pre-built musl binary for your architecture
and install it:

```bash
# Detect architecture
ARCH=$(uname -m)
case "${ARCH}" in
  x86_64)  ARCH_LABEL="x86_64" ;;
  aarch64) ARCH_LABEL="aarch64" ;;
  *)       echo "Unsupported architecture: ${ARCH}"; exit 1 ;;
esac

# Fetch the latest release tarball
curl -fsSL \
  "https://github.com/olafkfreund/ravn-agents/releases/latest/download/ravn-${ARCH_LABEL}-static.tar.gz" \
  | sudo tar -xz -C /usr/local/bin \
      "ravnd-${ARCH_LABEL}" \
      "ravn-actuator-${ARCH_LABEL}"

# Rename to canonical names
sudo mv /usr/local/bin/ravnd-${ARCH_LABEL}        /usr/local/bin/ravnd
sudo mv /usr/local/bin/ravn-actuator-${ARCH_LABEL} /usr/local/bin/ravn-actuator
sudo chmod 755 /usr/local/bin/ravnd /usr/local/bin/ravn-actuator
```

Verify the binaries are static (no dynamic library dependencies):

```bash
ldd /usr/local/bin/ravnd        # expected: "not a dynamic executable"
ldd /usr/local/bin/ravn-actuator
```

---

## Step 3 — Create a minimal agent config

```bash
sudo mkdir -p /etc/ravn

# Replace CONTROL_PLANE_IP with the IP or hostname of your control-plane host.
CONTROL_PLANE="http://CONTROL_PLANE_IP:18090"
NATS_URL="nats://CONTROL_PLANE_IP:4222"

sudo tee /etc/ravn/ravn-agent.toml >/dev/null <<EOF
[server]
url = "${NATS_URL}"

[log]
level = "info"

[detection]
journald.enable = true
failed_units.enable = true
auth.enable = true
updates.enable = true

[enrollment]
endpoint = "${CONTROL_PLANE}"
EOF
```

> For a production install you will also set a `bootstrap_token_file` pointing
> to a token file and configure mTLS enrollment — see the enrollment section
> of the architecture docs.  For this quickstart, enrollment is unauthenticated
> (no token required).

---

## Step 4 — Install and start the systemd unit

```bash
# Install the unit file
sudo curl -fsSL \
  "https://github.com/olafkfreund/ravn-agents/releases/latest/download/ravn-systemd-units.tar.gz" \
  | sudo tar -xz --strip-components=1 \
      -C /etc/systemd/system \
      dist/systemd/ravnd.service

# Reload systemd and enable the agent
sudo systemctl daemon-reload
sudo systemctl enable --now ravnd.service

# Confirm it started cleanly
sudo systemctl status ravnd.service
journalctl -u ravnd.service -n 30 --no-pager
```

If the service is active and you see log lines like
`detected 0 failed units` or `agent enrolled`, you're done.

---

## Step 5 — Trigger a test failure and watch it heal

This confirms the full detection + remediation loop works end-to-end.

### 5a — Enable remediation on the agent (optional but recommended)

```bash
# Create the "ravn" group that the actuator socket is owned by
sudo groupadd -r ravn 2>/dev/null || true

# Install the actuator unit
sudo curl -fsSL \
  "https://github.com/olafkfreund/ravn-agents/releases/latest/download/ravn-systemd-units.tar.gz" \
  | sudo tar -xz --strip-components=1 \
      -C /etc/systemd/system \
      dist/systemd/ravn-actuator.service

# Tell the actuator the signing key (printed during enrollment)
# Replace the placeholder with the real key from your control-plane logs.
sudo mkdir -p /etc/ravn
sudo tee /etc/ravn/actuator.env >/dev/null <<'EOF'
RAVN_ACTUATOR_SOCKET=/run/ravn/actuator.sock
RAVN_COMMAND_PUBKEY=REPLACE_WITH_BASE64_ED25519_PUBKEY
EOF
sudo chmod 600 /etc/ravn/actuator.env

# Add an EnvironmentFile line to the actuator unit (drop-in)
sudo mkdir -p /etc/systemd/system/ravn-actuator.service.d
sudo tee /etc/systemd/system/ravn-actuator.service.d/env.conf >/dev/null <<'EOF'
[Service]
EnvironmentFile=/etc/ravn/actuator.env
EOF

# Enable ravnd to reach the actuator socket via the ravn group
sudo mkdir -p /etc/systemd/system/ravnd.service.d
sudo tee /etc/systemd/system/ravnd.service.d/remediation.conf >/dev/null <<'EOF'
[Service]
Environment=RAVN_REMEDIATION=1
Environment=RAVN_ACTUATOR_SOCKET=/run/ravn/actuator.sock
Environment=RAVN_COMMAND_POLL_SECS=10
SupplementaryGroups=systemd-journal ravn
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now ravn-actuator.service
sudo systemctl restart ravnd.service
```

### 5b — Create a crasher unit

```bash
sudo tee /etc/systemd/system/ravn-crasher.service >/dev/null <<'EOF'
[Unit]
Description=Ravn demo crasher — fails immediately

[Service]
ExecStart=/bin/false
Restart=no
EOF

sudo systemctl daemon-reload
sudo systemctl start ravn-crasher.service || true  # it will fail by design
systemctl is-failed ravn-crasher.service && echo "unit is in failed state — expected"
```

### 5c — Watch the portal

Open `http://CONTROL_PLANE_IP:18088` in your browser.  Within a few seconds:

1. The agent detects `ravn-crasher.service` entering the **failed** state.
2. The event appears in the **Events** view with an AI-generated explanation.
3. If remediation is enabled and the control plane approves a `systemctl reset-failed ravn-crasher` command, the unit's failed state is cleared — visible in the **Remediations** tab.

---

## Upgrading

To upgrade the agent binary in place:

```bash
sudo systemctl stop ravnd.service
sudo mv /usr/local/bin/ravnd /usr/local/bin/ravnd.bak
# download + install new binary as in Step 2
sudo systemctl start ravnd.service
```

---

## Uninstall

```bash
sudo systemctl disable --now ravnd.service ravn-actuator.service
sudo rm -f /etc/systemd/system/ravnd.service \
           /etc/systemd/system/ravn-actuator.service \
           /etc/systemd/system/ravn-crasher.service
sudo rm -rf /etc/systemd/system/ravnd.service.d \
            /etc/systemd/system/ravn-actuator.service.d
sudo rm -f /usr/local/bin/ravnd /usr/local/bin/ravn-actuator
sudo rm -rf /etc/ravn
sudo systemctl daemon-reload
```

---

## Troubleshooting

| Symptom | Check |
|---------|-------|
| `ravnd` fails to start | `journalctl -u ravnd -n 50` — config parse errors appear here |
| Agent not visible in portal | Verify `NATS_URL` in the config; check firewall rules for port 4222 |
| `ProtectSystem=strict` blocks a write | Add a `ReadWritePaths=` drop-in for the specific path |
| Actuator not healing | Confirm `RAVN_COMMAND_PUBKEY` matches the key on the control plane; check `journalctl -u ravn-actuator` |
| `DynamicUser` not supported | Your systemd is older than 232; see the comment in `ravnd.service` |

---

## Next steps

- [Architecture overview](./architecture.md) — how the detection and remediation pipeline works
- [Runbook authoring guide](./runbook-authoring-guide.md) — write per-service remediation playbooks
- [NixOS module](../nixos/modules/agent.nix) — the declarative equivalent of this guide
