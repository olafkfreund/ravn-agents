# Ravn self-healing demo

A self-contained, repeatable demo that shows the whole Ravn story on one laptop:

- a **NixOS-style host** (a systemd container) running `ravnd` + the privileged
  actuator + a `flaky` unit;
- a self-contained **Ollama sidecar** (optionally GPU-accelerated) that turns
  raw events into plain-language explanations — host events on the agent, K8s
  events on the control plane;
- a **k3d cluster** (5–10 pods) running the Ravn **controller + node-agent**
  with a spread of healthy and failing workloads;
- the **portal** topology showing the cluster (☸) and the host (❄) as two
  distinct groups;
- a live **self-healing remediation**: kill a unit → the control plane proposes a
  fix → you approve → the agent heals it, at-most-once, with a signed command.

Everything runs on one shared docker network so the in-cluster pods reach the
control plane's NATS by name (no host networking) — the same trick as
`scripts/e2e-k8s.sh`.

## Prerequisites

- Docker, [`k3d`](https://k3d.io) (v5+), `kubectl`, `python3`
- That's it for the LLM — the demo runs its **own Ollama in a sidecar** and pulls
  the model (`qwen3:1.7b`) into a named volume on first start. You do **not** need
  a host Ollama. See [LLM explanations & GPU](#llm-explanations--gpu) to put it on
  a GPU.

## Bring it up

```sh
scripts/demo-up.sh
```

This is **idempotent** — re-run it any time. It always brings the control plane
up *with* the demo overlay (`docker-compose.demo.yml`), which is what enables
remediation templates, the command-signing key, and Ollama explanations.

> ⚠️ Don't start the stack with a bare `docker compose up` — without the overlay
> the control plane silently disables remediation (no templates dir). Always go
> through `scripts/demo-up.sh`, or pass both files explicitly:
> `docker compose -f docker-compose.yml -f demo/docker-compose.demo.yml up -d`.

When it finishes:

- Portal (built image): <http://localhost:8088/topology>
- Portal (dev server): `cd portal && npm run dev` → <http://localhost:5318/topology>
- Control plane API: <http://127.0.0.1:18090>

## What you'll see

**Topology** auto-groups by `kind`:

- ☸ **k3d-cluster** — `k3d-demo` (the controller agent for the cluster)
- ❄ **nixos** — `demo-nixos-host` (the systemd host)

The k3d agents and the host agent are labelled (`kind`, `env`) so the groups —
and their icons — are distinct at a glance.

**Events** stream real Kubernetes signals from the cluster — `CrashLoopBackOff`
(payments), `OOMKilled` (cache), `ImagePullBackOff` (reporting) — each enriched
with a plain-language LLM explanation. Click any event to read it.

## LLM explanations & GPU

Ravn attaches a plain-language **explanation** (and a suggested check) to events
via an OpenAI-compatible inference endpoint. Two paths feed the same demo Ollama:

| Events | Explained by | Endpoint env |
|---|---|---|
| **Host** (failed units, journald) | the host agent, inline before publish | `RAVN_INFERENCE_ENDPOINT` on `ravnd` |
| **Kubernetes** (workload/node) | the control plane, async after ingest | `RAVN_INFERENCE_ENDPOINT` on `control-plane` |

The demo overlay runs a self-contained **Ollama sidecar** (`ollama` service) so
nothing on your host is required; `scripts/demo-up.sh` pulls `qwen3:1.7b` into
the `ollama-demo` volume and warms it. Override the model with
`RAVN_DEMO_MODEL=<tag>` (it's pulled automatically).

### GPU acceleration

`scripts/demo-up.sh` **auto-detects** the GPU and layers in the right overlay:
`/dev/kfd` → AMD ROCm, else `nvidia-smi` → NVIDIA, else CPU. Force it with
`RAVN_DEMO_GPU=rocm|nvidia|cpu`. Confirm what Ollama chose:

```sh
docker compose -p ravn-agents -f docker-compose.yml -f demo/docker-compose.demo.yml \
  -f demo/docker-compose.gpu-rocm.yml exec ollama ollama ps   # PROCESSOR column → "100% GPU"
```

**AMD (ROCm)** — `demo/docker-compose.gpu-rocm.yml`. Uses the `ollama/ollama:rocm`
image and maps `/dev/kfd` + `/dev/dri`. Two host-specific knobs:

- **Render/video group GIDs.** The container must join the groups that own
  `/dev/dri/renderD*`. Find them with `getent group render video` and export
  `RAVN_RENDER_GID` / `RAVN_VIDEO_GID` if they differ from the defaults (303/26).
- **GFX version.** RDNA-class cards often need `HSA_OVERRIDE_GFX_VERSION`
  (`11.0.0` for gfx1100 / RX 7900). Override with
  `RAVN_HSA_OVERRIDE_GFX_VERSION`, and pick a card with `RAVN_ROCR_VISIBLE_DEVICES`.

```sh
RAVN_DEMO_GPU=rocm RAVN_RENDER_GID=303 RAVN_VIDEO_GID=26 scripts/demo-up.sh
```

**NVIDIA (CUDA)** — `demo/docker-compose.gpu-nvidia.yml`. The stock
`ollama/ollama` image already bundles CUDA; you only need the host driver +
[NVIDIA Container Toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/).

```sh
RAVN_DEMO_GPU=nvidia scripts/demo-up.sh
```

**CPU only** — the default when no GPU is found. Works fine; `qwen3:1.7b` answers
in a few seconds on a modern CPU, just slower under the demo's event load.

### Other options

- **Other GPUs / runtimes.** Intel or other cards can work via Ollama's Vulkan
  backend — start from `demo/docker-compose.gpu-rocm.yml`, drop the ROCm-only
  bits, map `/dev/dri`, and set `OLLAMA_VULKAN=1`. (Not tested in this repo.)
- **A bigger model.** `RAVN_DEMO_MODEL=qwen3:14b scripts/demo-up.sh` — slower,
  more detailed. Anything Ollama can run works.
- **Reuse an existing/remote Ollama.** Skip the sidecar and point the services at
  it: set `RAVN_INFERENCE_ENDPOINT` on `control-plane` (and `ravnd`) to your
  endpoint (e.g. `http://host.docker.internal:11434/v1`). Note a host Ollama bound
  to `127.0.0.1` is **not** reachable from containers — bind it to the docker
  bridge (`OLLAMA_HOST=0.0.0.0`) first, which is exactly the loopback limitation
  the sidecar sidesteps.

> **Shared-kernel note.** Because the host container and the k3d cluster share
> the laptop's kernel, the cluster's OOM/stack-trace spam would otherwise land in
> the host agent's journald feed. The demo sets `RAVN_JOURNALD_SKIP_KERNEL=1` so
> the host feed stays focused on service health. On real, separate hosts this
> doesn't arise.

## Test the remediation feature

```sh
scripts/demo-remediate.sh          # fail flaky.service, print the proposal id
scripts/demo-remediate.sh --auto   # …and approve + wait for the heal (hands-off)
```

Or do it by hand to narrate the loop:

1. **Fail a unit** on the host:
   ```sh
   docker compose -p ravn-agents -f docker-compose.yml -f demo/docker-compose.demo.yml \
     exec host-agent systemctl kill -s SIGKILL flaky.service
   ```
   (First `systemctl start flaky.service` and wait ~7s if it's already failed —
   the failed-unit tap only fires on a *transition* into `failed`, polling every 5s.)
2. **Watch the proposal** appear on the portal's **Remediations** page (or
   `GET /api/remediations`). It matched the `failed-unit-restart` template.
3. **Approve** it (button on the page, or
   `POST /api/remediations/<id>/approve`). The control plane issues an
   Ed25519-**signed** command.
4. The agent **pulls** the signed command, the privileged actuator **restarts**
   the unit, and the result (`succeeded`, `observed_state: active`) is reported
   back — recorded at-most-once in the idempotency ledger.

## Tear down

```sh
k3d cluster delete ravn-demo
docker compose -p ravn-agents -f docker-compose.yml -f demo/docker-compose.demo.yml down -v
```

## Security note

`demo/command.key` and `demo/command_pubkey.b64` are a **throwaway test
keypair** committed only so the demo signs/verifies out of the box. Never reuse
them anywhere real — a production control plane generates its own key and pins
the agents' public keys via enrollment.
