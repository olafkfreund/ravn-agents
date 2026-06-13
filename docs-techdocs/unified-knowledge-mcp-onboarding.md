# Onboarding Guide: Unified Knowledge MCP Server

The **Unified Knowledge MCP Server** (`scripts/knowledge-mcp.py`) is a federated Model Context Protocol (MCP) server that connects your AI coding assistant (e.g., Claude Code, Cursor, or an Antigravity SDK Agent) directly to your internal software catalog and documentation hubs.

When an infrastructure or code failure is detected, the agent can use this server to search runbooks, understand service dependencies, and retrieve on-call details across multiple documentation backends in parallel.

---

## 1. Supported Knowledge Connectors

The server automatically aggregates and searches information from:
1. **Developer Portals (Backstage):** Queries the Backstage Catalog REST API to fetch service owner specs, lifecycle data, and TechDocs annotations.
2. **Wikis (Confluence / Notion):** Queries wiki pages via CQL search or Notion databases.
3. **Local Docs (GitHub Pages & TechDocs):** Recursively searches markdown source directories in the repository (such as `docs/` and `docs-techdocs/`).
4. **GitHub Wikis:** Recursively searches a local clone of a GitHub Wiki repository.
5. **Built-in Runbooks:** Provides direct fallbacks for demo/testing workloads (`frontend`, `db`, `crasher`, `flaky.service`, `fwupd-refresh.service`).

---

## 2. Configuration & Portal Settings (Preferred)

Ravn supports configuring all documentation and knowledge source settings directly in the **System Settings** page of the Portal UI. Using the Portal is the recommended approach as settings are stored securely in `settings.json` and loaded dynamically by the SRE agents and MCP servers without requiring restarts.

### Portal Settings Panel
Navigate to the Portal (e.g., `http://localhost:5318/settings` in development) to manage:
- **Local Runbooks Directory:** Path to your local markdown runbooks.
- **Local GitHub Wiki Directory:** Path to local clone of your GitHub Wiki.
- **Backstage Software Catalog:** Toggle integration, specify Backstage URL, and optional API Bearer token.
- **Confluence Cloud Spaces:** Toggle integration, specify Confluence site URL, API username, and API token.
- **Notion Workspace:** Toggle integration and specify Notion API key.

### Environment Variable Fallback
If you prefer not to use the Portal UI or need to override configurations for local development, you can still supply the following environment variables to the MCP server:

| Environment Variable | Description | Example |
| :--- | :--- | :--- |
| `LOCAL_RUNBOOKS_DIR` | Path to local markdown runbooks folder | `/path/to/project/runbooks` |
| `GITHUB_WIKI_DIR` | Path to local clone of GitHub Wiki | `/path/to/project/wiki` |
| `BACKSTAGE_URL` | Root URL of your Backstage instance | `http://backstage.internal.corp` |
| `BACKSTAGE_TOKEN` | Bearer Token for Backstage API (optional) | `pst-xyz123...` |
| `CONFLUENCE_URL` | URL of your Confluence site | `https://company.atlassian.net/wiki` |
| `CONFLUENCE_USER` | Confluence API username / email | `sre-agent@company.com` |
| `CONFLUENCE_TOKEN` | Atlassian API Token | `ATATT3xF...` |
| `NOTION_API_KEY` | Integration API key for Notion | `secret_notion...` |

---

## 3. Client Onboarding & Configuration

### A. Claude Code / Claude CLI
Add this configuration block to your global Claude CLI config file (typically `~/.config/claude/config.json`):
```json
{
  "mcpServers": {
    "unified-knowledge": {
      "command": "python3",
      "args": ["/home/olafkfreund/Source/GitHub/ravn-agents/scripts/knowledge-mcp.py"],
      "env": {
        "LOCAL_RUNBOOKS_DIR": "/home/olafkfreund/Source/GitHub/ravn-agents/runbooks",
        "BACKSTAGE_URL": "http://backstage.internal.corp",
        "CONFLUENCE_URL": "https://company.atlassian.net/wiki",
        "CONFLUENCE_USER": "sre-agent@company.com",
        "CONFLUENCE_TOKEN": "ATATT3xF..."
      }
    }
  }
}
```

### B. Antigravity SDK Agent
To initialize a custom agent in python using the Google Antigravity SDK:
```python
import asyncio
from google.antigravity import Agent, LocalAgentConfig, types

async def main():
    mcp_servers = [
        types.McpStdioServer(
            name="unified-knowledge",
            command="python3",
            args=["/home/olafkfreund/Source/GitHub/ravn-agents/scripts/knowledge-mcp.py"],
            env={
                "LOCAL_RUNBOOKS_DIR": "/home/olafkfreund/Source/GitHub/ravn-agents/runbooks"
            }
        )
    ]
    
    config = LocalAgentConfig(mcp_servers=mcp_servers)
    
    async with Agent(config) as agent:
        response = await agent.chat("Search for Confluence runbooks for the 'frontend' service database errors.")
        print(await response.text())

if __name__ == "__main__":
    asyncio.run(main())
```

---

## 4. Real-life Troubleshooting Examples

### Example 1: Searching for Kubernetes Pod Failures
**User query to the Agent:**
> *"The pod `frontend` is throwing database connection errors in namespace `ravn-test`. Look up what might be wrong and who to contact."*

**Under the Hood (Tool Execution):**
1. The Agent calls the `search_runbooks` tool:
   ```json
   {
     "name": "search_runbooks",
     "arguments": {
       "query": "database connection error",
       "service": "frontend"
     }
   }
   ```
2. The MCP server returns matching runbook snippets from **Backstage TechDocs** and **Confluence**:
   ```
   ### Frontend Service Runbook (Backstage TechDocs)
   Owner/On-call: team-frontend (on-call: @alice)
   
   If logs show 'Database unreachable!':
   1. Check if the database service is running: kubectl get pods -n ravn-test -l app=ravn-db
   2. If db pod is Terminating, wait for it to restart.
   ```
3. The agent presents a structured analysis to the user, complete with the diagnostic commands and on-call contact details.

### Example 2: Inspecting Systemd Unit Failure
**User query to the Agent:**
> *"The service `fwupd-refresh.service` keeps exiting with exit code 1. Is this critical?"*

**Under the Hood (Tool Execution):**
1. The Agent calls `search_runbooks(query="fwupd-refresh.service exit status 1")`.
2. The MCP server matches the built-in systemd wiki runbook:
   ```
   ### Fwupd Refresh Service Hardware Fault (Systemd Wiki)
   Owner: team-platform
   
   Safe to ignore in virtualization / cloud environments. The service fails because no physical firmware updates are supported on VMs.
   ```
3. The agent advises the user: *"You can safely ignore this restart failure because this is a virtualized development environment, which lacks physical firmware update support."*
