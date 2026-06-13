#!/usr/bin/env node
/**
 * Ravn MCP Server
 * Zero-dependency Model Context Protocol (MCP) server running over stdio.
 * Interfaces with the Ravn control plane on http://127.0.0.1:18080.
 */

const readline = require("readline");

const API_BASE = process.env.RAVN_API || "http://127.0.0.1:18080";

function logDebug(msg) {
  process.stderr.write(`[DEBUG] ${msg}\n`);
}

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false
});

rl.on("line", async (line) => {
  if (!line.trim()) return;
  try {
    const request = JSON.parse(line);
    logDebug(`Received request method: ${request.method}, id: ${request.id}`);
    
    // Handle JSON-RPC requests
    if (request.method && request.id !== undefined) {
      const response = await handleRequest(request);
      if (response) {
        process.stdout.write(JSON.stringify(response) + "\n");
      }
    }
  } catch (err) {
    logDebug(`Error processing line: ${err.message}`);
  }
});

async function handleRequest(req) {
  const { id, method, params } = req;

  switch (method) {
    case "initialize":
      return {
        jsonrpc: "2.0",
        id,
        result: {
          protocolVersion: "2024-11-05",
          capabilities: {
            tools: {}
          },
          serverInfo: {
            name: "ravn-mcp",
            version: "1.0.0"
          }
        }
      };

    case "notifications/initialized":
      logDebug("Client initialized protocol");
      return null;

    case "tools/list":
      return {
        jsonrpc: "2.0",
        id,
        result: {
          tools: [
            {
              name: "list_agents",
              description: "List all registered agents in the Ravn fleet, showing their hostnames, status (online/stale/offline), labels, and last seen timestamps.",
              inputSchema: {
                type: "object",
                properties: {}
              }
            },
            {
              name: "list_recent_events",
              description: "List the most recent security/infra events recorded by the Ravn control plane.",
              inputSchema: {
                type: "object",
                properties: {
                  limit: {
                    type: "integer",
                    description: "Maximum number of events to fetch (default: 50)",
                    minimum: 1,
                    maximum: 200
                  }
                }
              }
            },
            {
              name: "list_remediations",
              description: "List all recent remediation proposals, showing which ones are pending approval.",
              inputSchema: {
                type: "object",
                properties: {}
              }
            },
            {
              name: "approve_remediation",
              description: "Approve a pending remediation proposal by its UUID.",
              inputSchema: {
                type: "object",
                properties: {
                  proposal_id: {
                    type: "string",
                    description: "The UUID of the remediation proposal to approve"
                  },
                  approver: {
                    type: "string",
                    description: "Name of the operator approving the remediation",
                    default: "mcp-operator"
                  }
                },
                required: ["proposal_id"]
              }
            }
          ]
        }
      };

    case "tools/call":
      try {
        const toolResult = await callTool(params.name, params.arguments || {});
        return {
          jsonrpc: "2.0",
          id,
          result: toolResult
        };
      } catch (err) {
        logDebug(`Error calling tool ${params.name}: ${err.message}`);
        return {
          jsonrpc: "2.0",
          id,
          error: {
            code: -32603,
            message: err.message
          }
        };
      }

    default:
      return {
        jsonrpc: "2.0",
        id,
        error: {
          code: -32601,
          message: `Method not found: ${method}`
        }
      };
  }
}

async function callTool(name, args) {
  switch (name) {
    case "list_agents": {
      const res = await fetch(`${API_BASE}/api/agents`);
      if (!res.ok) throw new Error(`HTTP error fetching agents: ${res.statusText}`);
      const data = await res.json();
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(data, null, 2)
          }
        ]
      };
    }

    case "list_recent_events": {
      const limit = args.limit || 50;
      const res = await fetch(`${API_BASE}/api/events?limit=${limit}`);
      if (!res.ok) throw new Error(`HTTP error fetching events: ${res.statusText}`);
      const data = await res.json();
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(data, null, 2)
          }
        ]
      };
    }

    case "list_remediations": {
      const res = await fetch(`${API_BASE}/api/remediations`);
      if (!res.ok) {
        // Fallback if remediation endpoint is not yet fully active
        return {
          content: [{ type: "text", text: "No active remediations found, or endpoint not configured." }]
        };
      }
      const data = await res.json();
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(data, null, 2)
          }
        ]
      };
    }

    case "approve_remediation": {
      const { proposal_id, approver } = args;
      const res = await fetch(`${API_BASE}/api/remediations/${proposal_id}/approve`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ approver: approver || "mcp-operator" })
      });
      if (!res.ok) throw new Error(`HTTP error approving remediation: ${res.statusText}`);
      const data = await res.json();
      return {
        content: [
          {
            type: "text",
            text: `Successfully approved remediation proposal ${proposal_id}: ${JSON.stringify(data)}`
          }
        ]
      };
    }

    default:
      throw new Error(`Unknown tool: ${name}`);
  }
}
