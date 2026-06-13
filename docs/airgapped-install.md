---
layout: page
title: Air-Gapped Install Guide
permalink: /airgapped-install/
nav_order: 6
---

# Air-Gapped Install Guide

This document covers deploying Ravn in a network-isolated ("air-gapped")
environment — one where hosts have **no outbound internet access**.  Local
inference is where Ravn earns its keep in this scenario: detection, explanation,
and remediation all run from local binaries and model weights, with zero calls to
any external API.

---

## What "air-gapped" means for Ravn

Every component in the default Ravn path is already offline-capable **except**
for one optional startup call: when OIDC/JWKS authentication is configured with
a URL source (`RAVN_OIDC_JWKS_URL` or `RAVN_INGEST_OIDC_JWKS_URL`), the control
plane fetches the IdP's JWKS document over HTTPS on startup.

Setting `RAVN_AIRGAPPED=1` blocks that fetch at startup with a clear error,
forcing the operator to supply the JWKS as a local file instead.

### Full audit of network calls

The following table lists every place in the Ravn codebase that can make an
outbound network call, and how each is gated in air-gapped mode.

| Component | Call | Trigger | Air-gapped gate |
|---|---|---|---|
| `ravn-server` | Fetch JWKS document | `RAVN_OIDC_JWKS_URL` or `RAVN_INGEST_OIDC_JWKS_URL` set | **Blocked at startup** when `RAVN_AIRGAPPED=1`; use `_FILE` variant instead |
| `ravn-server` | Kubernetes TokenReview | `RAVN_INGEST_TOKENREVIEW_URL` set | Optional; disabled by default; not needed in air-gapped K8s when JWKS file is provided |
| `ravn-server` | OpenAI-compatible inference | `RAVN_INFERENCE_ENDPOINT` set | Operator-controlled URL — point at the internal llama-server, not an external API |
| `ravn-agent` | Enrollment (mTLS) | `RAVN_ENROLL_ENDPOINT` + `RAVN_ENROLL_TOKEN_FILE` set | Calls the **internal** control plane, not the internet; idempotent after first boot |
| `ravn-agent` | Inference explanations | `RAVN_INFERENCE_ENDPOINT` set | Calls the **local** llama-server on 127.0.0.1; loopback only |
| `ravn-agent` | Command pull (remediation) | `RAVN_REMEDIATION=1` set | Calls the **internal** control plane; no external endpoint |
| `ravn-agent` | NixOS generation diff | `nix store diff-closures` subprocess | Reads local Nix store only; no downloads |
| `ravn-k8s` | HTTP event ingest | `RAVN_INGEST_URL` set | Points at internal control plane |
| `ravn-eval` | Benchmark inference | CLI tool, not a daemon | Calls local llama-server endpoint |
| NixOS module | Model fetch | `services.ravn.agent.inference.model.url` set | Use `model.path` instead — the `url` field triggers `pkgs.fetchurl` at Nix **eval time**, not runtime, but it requires internet on the build host |
| Nix flake | `nixpkgs` input | `nix flake update` | Locked in `flake.lock`; transfers via `nix copy`, not live internet |

No telemetry, update-check, or phone-home calls exist in the Ravn codebase.

---

## Prerequisites

### 1. Build host (has internet access)

You need one machine that can reach the internet to build the closure and
download model weights.  This machine never joins the air-gapped network.

Install Nix with flake support:

```bash
curl -fsSL https://install.determinate.systems/nix | sh -s -- install
```

### 2. Model weights

Ravn uses GGUF models served by `llama-server` (from
[llama.cpp](https://github.com/ggerganov/llama.cpp)).  Download the model on
your build host:

```bash
# Example: Qwen3 1.7B Q4_K_M (good quality/speed trade-off for CPU inference)
wget https://huggingface.co/bartowski/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q4_K_M.gguf \
     -O qwen3-1.7b-q4_k_m.gguf
```

The model file is **not** managed by Nix in the air-gapped path — it is a large
binary artifact that you transfer separately (see step 4 below).

---

## Step 1 — Generate secrets (on the build host)

All secrets are generated on the build host and transferred to the air-gapped
hosts out-of-band.  They are **never** stored in the Nix store.

```bash
mkdir -p airgap-secrets
cd airgap-secrets

# 1a. Internal CA keypair for mTLS agent enrollment.
openssl genrsa -out ca.key 4096
openssl req -x509 -new -nodes -key ca.key -sha256 -days 3650 \
    -subj "/CN=Ravn Internal CA" -out ca.crt

# 1b. Bootstrap token — agents present this once to receive their mTLS cert.
openssl rand -base64 32 | tr -d '\n' > bootstrap-token

# 1c. Control-plane command-signing key (Ed25519).
#     The public key is advertised to agents at enrollment; the server signs
#     every remediation command with the private key.
nix run github:olafkfreund/ravn-agents#ravn-server -- --gen-command-key \
    --out command.key 2>/dev/null || \
    openssl genpkey -algorithm ed25519 -out command.key
```

> **Security note:** `ca.key` and `bootstrap-token` must never be stored in
> the Nix store, in version control, or in environment variables visible to
> unprivileged processes.  Use systemd credentials, agenix, or a hardware HSM.

---

## Step 2 — Build the Nix closure (on the build host)

```bash
# Clone the repo on the build host.
git clone https://github.com/olafkfreund/ravn-agents
cd ravn-agents

# Build all binaries and the NixOS system closures for your target architecture.
nix build .#ravn-server .#ravn-agent .#ravn-actuator

# Build the system closures for your air-gapped hosts.
# Adjust the configuration names to match what you have in flake.nix.
nix build .#nixosConfigurations.airgap-control-plane.config.system.build.toplevel
nix build .#nixosConfigurations.airgap-agent.config.system.build.toplevel
```

---

## Step 3 — Transfer the closure to the air-gapped hosts

Use a USB drive, a one-way data diode, or an approved transfer mechanism:

```bash
# Export to a single archive on the build host.
nix copy --to "file:///media/usbdrive/nix-export" \
    .#nixosConfigurations.airgap-control-plane.config.system.build.toplevel \
    .#nixosConfigurations.airgap-agent.config.system.build.toplevel

# On the air-gapped control-plane host (USB mounted at /media/usb):
nix copy --from "file:///media/usb/nix-export" \
    /run/current-system   # or the specific store path

# Alternatively, use `nix-store --export` / `--import` for older setups:
nix-store --export $(nix-store -qR result) | gzip > ravn-closure.nar.gz
# Transfer and on the target:
gunzip < ravn-closure.nar.gz | nix-store --import
```

### For Docker/OCI deployments

```bash
# On the build host (with internet access):
docker compose -f demo/docker-compose.airgapped.yml build
docker save \
    postgres:17-alpine \
    nats:2-alpine \
    ravn-server:airgap \
    ravn-agent:airgap \
    | gzip > ravn-airgap-images.tar.gz

# Transfer ravn-airgap-images.tar.gz to the air-gapped host, then:
docker load < ravn-airgap-images.tar.gz
```

---

## Step 4 — Transfer model weights

```bash
# On the build host: copy the GGUF file to the transfer medium.
cp qwen3-1.7b-q4_k_m.gguf /media/usbdrive/

# On the air-gapped agent host:
mkdir -p /var/lib/ravn/models
cp /media/usb/qwen3-1.7b-q4_k_m.gguf /var/lib/ravn/models/
chmod 644 /var/lib/ravn/models/qwen3-1.7b-q4_k_m.gguf
```

The NixOS module option `services.ravn.agent.inference.model.path` points at
this file.  Do **not** set `model.url` — it would trigger a download attempt on
the next `nixos-rebuild`.

---

## Step 5 — Deploy secrets to the hosts

Transfer the secrets from `airgap-secrets/` to each host using your approved
out-of-band mechanism (USB, hardware HSM, etc.):

### Control plane

```bash
# On the control-plane host:
mkdir -p /run/secrets
install -m 0400 -o root ca.key    /run/secrets/ravn-ca-key
install -m 0400 -o root bootstrap-token /run/secrets/ravn-bootstrap-token
install -m 0444 -o root ca.crt    /etc/ravn/ca.crt
```

With systemd credentials (recommended — secrets never land on disk):

```ini
# In the unit override:
[Service]
LoadCredential=ca-key:/run/secrets/ravn-ca-key
LoadCredential=enroll-token:/run/secrets/ravn-bootstrap-token
```

The NixOS control-plane module already wires `LoadCredential` when
`enrollment.caKeyFile` and `enrollment.bootstrapTokenFile` are set.

### Agent hosts

```bash
# On each agent host (bootstrap-token is only needed until enrolled):
mkdir -p /run/secrets
install -m 0400 -o root bootstrap-token /run/secrets/ravn-bootstrap-token
```

After the first successful enrollment the agent stores its mTLS credentials in
its `StateDirectory` and never contacts the enrollment endpoint again unless the
credentials are deleted.

---

## Step 6 — Configure and switch

### NixOS (recommended)

Reference `nixos/configurations/airgapped.nix` in your flake.  The two
`nixosConfigurations` entries there (`airgap-control-plane` and `airgap-agent`)
are the canonical starting points:

```nix
# In your flake.nix outputs:
nixosConfigurations = (import ./nixos/configurations/airgapped.nix {
  inherit self nixpkgs;
});
```

Then on each host:

```bash
nixos-rebuild switch --flake .#airgap-control-plane   # on the control-plane
nixos-rebuild switch --flake .#airgap-agent           # on each agent
```

### Docker Compose

```bash
cd ravn-agents

# Create the isolated network (no gateway).
docker network create --internal ravn-airgap

# Place secrets (or use Docker secrets).
mkdir -p secrets
cp airgap-secrets/ca.crt airgap-secrets/ca.key \
   airgap-secrets/bootstrap-token secrets/

# Place the model.
mkdir -p demo/models
cp qwen3-1.7b-q4_k_m.gguf demo/models/

# Start the stack.
docker compose -f demo/docker-compose.airgapped.yml up -d

# Verify the control plane is up.
docker compose -f demo/docker-compose.airgapped.yml exec control-plane \
    curl -sf http://127.0.0.1:8080/health

# Verify no outbound connectivity (should time out).
docker compose -f demo/docker-compose.airgapped.yml exec control-plane \
    curl -m 5 https://example.com || echo "correctly blocked"
```

---

## Internal CA for mTLS enrollment

In an air-gapped deployment, the internal CA signs all agent certificates.
There is no external PKI dependency.  The `ravn-server` CA module (`crates/ravn-server/src/ca.rs`)
implements the signing logic; the control plane is the only entity that holds
the CA private key.

The trust chain:

```
Internal CA (ca.crt / ca.key)
  └── Agent certificate (signed by control plane at enrollment)
        └── mTLS mutual authentication (agent ↔ control plane)
```

Agents pin the CA certificate at enrollment time.  If you rotate the CA, you
must re-enroll all agents (delete their credential directories and restart).

---

## OIDC / JWKS in an air-gapped environment

If you use an **internal IdP** (e.g. Keycloak, Dex, or Zitadel running inside
the air-gapped network):

1. Export the IdP's JWKS endpoint on a machine that can reach it:
   ```bash
   curl -sf https://idp.internal.example.com/realms/ravn/protocol/openid-connect/certs \
       > oidc-jwks.json
   ```

2. Transfer `oidc-jwks.json` to the control-plane host.

3. Configure the control plane with the file path, **not** a URL:
   ```bash
   RAVN_AIRGAPPED=1
   RAVN_OIDC_ISSUER=https://idp.internal.example.com/realms/ravn
   RAVN_OIDC_JWKS_FILE=/etc/ravn/oidc-jwks.json   # NOT RAVN_OIDC_JWKS_URL
   RAVN_OIDC_AUDIENCE=ravn-portal
   ```

4. When the IdP rotates signing keys, copy the updated JWKS file and restart
   `ravn-server` (key rotation is rare — on the order of months).

With `RAVN_AIRGAPPED=1`, the server will **refuse to start** if a `_URL`
variant is set, giving the operator a clear error message rather than a silent
timeout.

---

## Verification checklist

After deployment, verify the following:

- [ ] `systemctl is-active ravn-server` (or Docker service) is `active`
- [ ] `curl -sf http://<control-plane>:8080/health` returns `{"status":"ok"}`
- [ ] Agent appears in the portal under Agents (enrollment succeeded)
- [ ] Inference: `curl -sf http://127.0.0.1:18181/health` on the agent host
- [ ] No outbound connections from any Ravn process:
      `ss -tnp | grep -E 'ravn|llama'` shows only loopback and internal addresses
- [ ] `RAVN_AIRGAPPED=1` appears in `ravn-server` logs on startup
- [ ] Triggering a healable failure produces a remediation proposal in the portal

---

## Troubleshooting

### "RAVN_AIRGAPPED=1 is set but JWKS source is a URL"

You have `RAVN_OIDC_JWKS_URL` or `RAVN_INGEST_OIDC_JWKS_URL` set.  Replace
them with their `_FILE` equivalents pointing at a locally-copied JWKS document.

### "enrollment rejected (403)"

The bootstrap token on the agent does not match the one on the control plane.
Re-copy the token and restart the agent.

### "inference request failed" in agent logs

The model file is missing or the llama-server service failed to start.  Check:

```bash
systemctl status ravn-llama.service
ls -lh /var/lib/ravn/models/
```

### Agent credentials exist but control plane rejects mTLS

The internal CA certificate may have been replaced.  Delete the agent's
credential directory (`/var/lib/ravn-agent/credentials/`) and restart `ravnd`
to trigger re-enrollment.

---

## Related files

| File | Purpose |
|---|---|
| `nixos/configurations/airgapped.nix` | NixOS configuration examples |
| `demo/docker-compose.airgapped.yml` | Docker Compose air-gapped profile |
| `nixos/tests/airgapped.nix` | CI/VM test with outbound networking blocked |
| `crates/ravn-server/src/config.rs` | `is_airgapped()` helper |
| `crates/ravn-server/src/main.rs` | `load_jwks()` guard |
| `nixos/modules/agent.nix` | `inference.model.path` vs `.url` |
| `nixos/modules/control-plane.nix` | Enrollment CA wiring |
