# Backstage integration

Ravn is modelled in Backstage as a **System** (`ravn`) that groups its
Components, the control-plane **API**, and its backing **Resources**. Everything
is declared in [`catalog-info.yaml`](https://github.com/olafkfreund/ravn-agents/blob/main/catalog-info.yaml)
at the repo root, which Backstage discovers from GitHub.

## What's in the catalog

- **System** `ravn` — owns the TechDocs and the project links.
- **API** `ravn-control-plane-api` — generated from `portal/openapi.json`.
- **Components** — `ravn-server`, `ravnd`, `ravn-actuator`, `ravn-controller`,
  `ravn-portal`, `ravn-core`, `ravn-crypto`, wired with `providesApis` /
  `consumesApis` / `dependsOn` so the relationship graph is meaningful.
- **Resources** — `ravn-postgres`, `ravn-nats`.

## How the docs stay automatic

The published site under `docs/` is **Jekyll** (Liquid templating), which MkDocs
can't parse. So TechDocs has its own source directory, `docs-techdocs/`, built by
[`mkdocs.yml`](https://github.com/olafkfreund/ravn-agents/blob/main/mkdocs.yml).
It holds this landing page plus **symlinks** to the MkDocs-safe canonical files
(architecture, roadmap, contributing, security) and to **every design spec under
`plans/`**.

[`scripts/techdocs-sync.sh`](https://github.com/olafkfreund/ravn-agents/blob/main/scripts/techdocs-sync.sh)
regenerates those symlinks from the canonical sources, and the
**TechDocs CI workflow** (`.github/workflows/techdocs.yml`) runs it before every
build. Because `mkdocs.yml` has no hand-maintained `nav`, **a doc added to
`docs/` or `plans/` appears in Backstage automatically** on the next push — no
catalog edit required.

## Publishing (one-time setup)

TechDocs can be served two ways; this repo supports both:

1. **Built-in builder** — if your Backstage `techdocs.builder` is `local`, the
   backend builds these docs on demand from the repo. Nothing else is needed.
2. **External storage (recommended for production)** — set these repo
   **Actions variables/secrets** and the CI workflow will `generate` + `publish`
   on every push to `main`:

    | Kind | Name | Example |
    |------|------|---------|
    | Variable | `TECHDOCS_PUBLISH` | `true` |
    | Variable | `TECHDOCS_PUBLISHER_TYPE` | `awsS3` / `googleGcs` / `azureBlobStorage` |
    | Variable | `TECHDOCS_STORAGE_NAME` | your bucket/container name |
    | Variable | `TECHDOCS_AWS_REGION` | `eu-north-1` (S3 only) |
    | Secret | `TECHDOCS_AWS_ACCESS_KEY_ID` / `TECHDOCS_AWS_SECRET_ACCESS_KEY` | … |

    Until `TECHDOCS_PUBLISH` is `true`, the workflow still **builds** the docs on
    every PR as a validation check — it just doesn't publish.
