# Ravn MCP Server

The Ravn MCP server (`ravn-mcp`) is a supported, versioned [Model Context
Protocol](https://modelcontextprotocol.io) bridge to the Ravn control plane. It
lets MCP-aware AI clients — Claude Code, Claude Desktop, and any other MCP host —
inspect your fleet (agents, events, topology, and remediation proposals) through
a small, audited tool surface.

It speaks JSON-RPC 2.0 over **stdio**, reuses the shared `ravn-core` types, and
calls the same HTTP API the portal uses. It ships with every Ravn release as the
`ravn-mcp` binary and OCI image.

> **It replaces the old prototypes.** Earlier `scripts/ravn-mcp.js` and
> `scripts/ravn_mcp_server.py` were throwaway experiments. They are gone; this
> crate (`crates/ravn-mcp`) is the one supported implementation.

---

## Doctrine: read-only by default

This is the central design rule, and it mirrors Ravn's broader remediation
doctrine (deterministic detection, human- or policy-approved actions, no LLM in
the action path):

- **Read-only by default.** Out of the box, the server exposes only tools that
  observe. It cannot change anything.
- **Mutations are opt-in and gated.** The two mutating tools are exposed *only*
  when you start the server with `--allow-mutations` **and** an admin-scoped
  token.
- **Never auto-executes.** Even with mutations enabled, a tool call only does
  what a human clicking a button in the portal would do: it asks the control
  plane to sign and enqueue a command for the agent to pull. The MCP server
  never executes a remediation itself, and there is no auto-approve path.

---

## Tools

### Read-only (always available)

| Tool | What it does |
| --- | --- |
| `list_agents` | All registered agents with status (`online`/`stale`/`offline`), host, labels, and last-seen time. |
| `list_recent_events` | Recent deterministic detection events, newest first. Optional `limit` (1–500, default 50). |
| `get_event` | A single event by `id` (UUID), including payload and any LLM explanation. |
| `get_topology` | The fleet shaped for the topology diagram, optionally grouped by a label key (`group_by`, e.g. `env`). |
| `list_remediation_proposals` | Remediation records, by default only those awaiting a decision (`pending_only`, default `true`). Read-only — does **not** approve or reject. |

### Mutating (gated behind `--allow-mutations` + admin token)

| Tool | What it does |
| --- | --- |
| `approve_remediation` | Approve a **pending** proposal by `id`: the control plane signs and enqueues the command, attributed to the supplied admin token. |
| `reject_remediation` | Reject a pending proposal by `id`. Records the rejection in the audit trail; no command is issued. |

Each tool carries honest MCP annotations (`readOnlyHint`, `destructiveHint`,
…) so clients can warn before invoking a state-changing tool.

---

## Configuration

The server is configured by flags or environment variables (a flag wins over its
env var, which wins over the default):

| Flag | Env var | Default | Meaning |
| --- | --- | --- | --- |
| `--url URL` | `RAVN_MCP_URL` | `http://127.0.0.1:8080` | Control-plane base URL. |
| `--token TOKEN` | `RAVN_MCP_TOKEN` | *(none)* | Bearer token for the control plane. A viewer token is enough for the read tools; mutations need an admin token. |
| `--allow-mutations` | `RAVN_MCP_ALLOW_MUTATIONS` | off | Expose the approve/reject tools. Requires an admin token. |
| *(n/a)* | `RAVN_MCP_LOG` / `RUST_LOG` | `warn` | Tracing filter. Logs go to **stderr** so stdout stays clean for the protocol. |

The bearer token maps to a role on the control plane via `RAVN_ADMIN_TOKEN` /
`RAVN_VIEWER_TOKEN` (or your portal OIDC groups). Safe (read) methods need a
viewer; mutating methods need admin. If the control plane has auth disabled, no
token is required.

---

## Setup: Claude Code

Register the server with the `claude mcp add` command (it speaks stdio, the
default transport):

```bash
# Read-only (recommended default)
claude mcp add ravn \
  --env RAVN_MCP_URL=https://ravn.internal.example.com \
  --env RAVN_MCP_TOKEN=$RAVN_VIEWER_TOKEN \
  -- ravn-mcp
```

Or add it by hand to your MCP client config (`.mcp.json` at the project root for
Claude Code, or the global Claude Code / Claude Desktop config). A read-only
entry:

```json
{
  "mcpServers": {
    "ravn": {
      "command": "ravn-mcp",
      "args": [],
      "env": {
        "RAVN_MCP_URL": "https://ravn.internal.example.com",
        "RAVN_MCP_TOKEN": "<viewer-token>"
      }
    }
  }
}
```

To enable the gated mutating tools, add `--allow-mutations` and use an **admin**
token. Do this deliberately — it lets the assistant approve/reject remediations:

```json
{
  "mcpServers": {
    "ravn": {
      "command": "ravn-mcp",
      "args": ["--allow-mutations"],
      "env": {
        "RAVN_MCP_URL": "https://ravn.internal.example.com",
        "RAVN_MCP_TOKEN": "<admin-token>"
      }
    }
  }
}
```

If `ravn-mcp` is not on your `PATH`, use an absolute path for `command`, run it
from the flake (`nix run github:olafkfreund/ravn-agents#ravn-mcp -- --url …`), or
point at the container image entrypoint.

---

## Setup: other MCP clients

Any MCP host that launches a stdio server works the same way: run `ravn-mcp` as
the server command, pass configuration through the environment, and let the host
drive the `initialize` → `tools/list` → `tools/call` handshake. The server
advertises protocol version `2025-06-18`.

You can sanity-check it by hand:

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  | ravn-mcp --url http://127.0.0.1:8080
```

You should see an `initialize` result followed by a `tools/list` result listing
the five read-only tools.

---

## Running from a release

The binary ships with the Nix flake and as an OCI image:

```bash
# Run the binary from the flake
nix run github:olafkfreund/ravn-agents#ravn-mcp -- --url http://127.0.0.1:8080

# Or load the container image
docker load < $(nix build github:olafkfreund/ravn-agents#ravn-mcp-image --print-out-paths)
```

The image has no exposed port — MCP is stdio, so the client attaches to the
container's stdin/stdout.

---

## Security notes

- **Prefer read-only.** Leave mutations off unless an assistant genuinely needs
  to drive approvals, and scope the token to exactly the role you intend.
- **Tokens are credentials.** Treat `RAVN_MCP_TOKEN` like any other secret;
  prefer your client's secret-injection over inlining it in committed config.
- **No shell, no arbitrary actions.** The tool surface is fixed at compile time.
  There is no generic "run this" tool, by design — the only state changes
  possible are approve/reject of pre-authored, deterministic remediation
  proposals, and only when explicitly enabled.
