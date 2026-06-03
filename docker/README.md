# Ravn — Docker / OCI

Two ways to get container images.

## One-command demo (no Nix)

From the repo root:

```bash
docker compose up --build
```

Brings up the control plane, NATS, Postgres, a demo agent, and the portal:

- **Portal** → <http://localhost:8088>
- **Control-plane API** → <http://localhost:18090> (e.g. `/api/events`, `/openapi.json`)

The demo agent has the host taps off (those need a real systemd host) and
instead watches `docker/watch/`. Edit `docker/watch/demo.conf` while it runs and
a **config-drift** event appears live in the portal. Each agent also heartbeats,
so it shows as **online** under Agents.

Stop with `Ctrl-C`; `docker compose down -v` removes the Postgres volume.

## Reproducible images (Nix)

```bash
docker load < $(nix build .#ravn-server-image --print-out-paths)
docker load < $(nix build .#ravn-agent-image  --print-out-paths)
```

These are minimal `dockerTools` layered images (just the binary + CA certs).

## Production notes

- Run real agents on the host via the NixOS module (`services.ravn.agent`) or a
  Kubernetes DaemonSet (epic #53) — containers can't see the host's
  systemd/journald, so the journald/auth/failed-unit/update taps need host
  access.
- Set strong Postgres credentials and front the API with TLS + auth (#26).
