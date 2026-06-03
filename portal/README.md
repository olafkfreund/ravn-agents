# Ravn Portal

Operator UI for the Ravn fleet — Vite + React + TypeScript, Tailwind, TanStack
Query, and a typed client generated from the control plane's OpenAPI spec.
Gruvbox theme with a light/dark toggle, matching the marketing site.

## Develop

```bash
pnpm install

# Point the dev proxy at your running control plane (use an uncommon port).
RAVN_BIND=127.0.0.1:18080 cargo run -p ravn-server   # in the repo root
VITE_API_PROXY=http://127.0.0.1:18080 pnpm dev        # http://localhost:5318
```

The Vite dev server (port 5318) proxies `/api`, `/health`, `/ready` and
`/openapi.json` to `VITE_API_PROXY` (default `http://127.0.0.1:8080`).

## Typed API client

`src/api/schema.d.ts` is generated from the server's OpenAPI document:

```bash
pnpm fetch:openapi   # curl $VITE_API_PROXY/openapi.json -> openapi.json
pnpm gen:api         # openapi-typescript openapi.json -> src/api/schema.d.ts
```

`src/lib/api.ts` wraps it with `openapi-fetch` for fully typed requests.

## Build

```bash
pnpm build           # tsc --noEmit && vite build  ->  dist/
```

## Scope

This is the M0 scaffold (#27): app shell, nav, theme toggle, and an Events
inventory table backed by `GET /api/events`. The live feed (#29), topology view
(#31–33), and category management (#30) build on this.
