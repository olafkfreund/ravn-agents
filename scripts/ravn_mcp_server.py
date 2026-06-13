#!/usr/bin/env python3
import json
import sys
import os
import urllib.request
import urllib.error

API_URL = os.environ.get("RAVN_API_URL", "http://127.0.0.1:18080")

def api_get(path):
    url = f"{API_URL.rstrip('/')}/{path.lstrip('/')}"
    try:
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req) as resp:
            return json.loads(resp.read().decode('utf-8'))
    except urllib.error.URLError as e:
        return {"error": f"Failed to connect to Ravn API at {url}: {e}"}
    except Exception as e:
        return {"error": f"Error querying Ravn API: {e}"}

def api_post(path, data=None):
    url = f"{API_URL.rstrip('/')}/{path.lstrip('/')}"
    try:
        payload = json.dumps(data or {}).encode('utf-8')
        req = urllib.request.Request(url, data=payload, method="POST")
        req.add_header("Content-Type", "application/json")
        with urllib.request.urlopen(req) as resp:
            return json.loads(resp.read().decode('utf-8'))
    except urllib.error.URLError as e:
        return {"error": f"Failed to connect to Ravn API at {url}: {e}"}
    except Exception as e:
        return {"error": f"Error calling Ravn API: {e}"}

def handle_list_tools():
    return {
        "tools": [
            {
                "name": "list_agents",
                "description": "Get the list of all registered Ravn agents, their hostnames, online/offline status, and labels.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "get_agent_details",
                "description": "Get details for a specific agent by ID, including its metadata, labels, and recent events.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "The UUID of the agent to query"
                        }
                    },
                    "required": ["agent_id"]
                }
            },
            {
                "name": "query_topology",
                "description": "Query the logical group topology of the agent fleet, grouping by a label like 'cluster' or 'env'.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "group_by": {
                            "type": "string",
                            "description": "The label key to group nodes by (e.g., 'cluster', 'env', or 'kind')"
                        }
                    }
                }
            },
            {
                "name": "list_remediations",
                "description": "List all active or past self-healing remediation proposals, including status and rationale.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "approve_remediation",
                "description": "Approve a pending self-healing remediation proposal by ID.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "remediation_id": {
                            "type": "string",
                            "description": "The UUID of the remediation proposal to approve"
                        }
                    },
                    "required": ["remediation_id"]
                }
            }
        ]
    }

def handle_call_tool(name, arguments):
    if name == "list_agents":
        data = api_get("/api/agents")
        return {"content": [{"type": "text", "text": json.dumps(data, indent=2)}]}
    
    elif name == "get_agent_details":
        agent_id = arguments.get("agent_id")
        if not agent_id:
            return {"content": [{"type": "text", "text": "Error: agent_id parameter is required"}]}
        agent = api_get(f"/api/agents/{agent_id}")
        events = api_get("/api/events")
        
        # Filter events for this agent
        agent_events = []
        if isinstance(events, list):
            agent_events = [e for e in events if e.get("agent_id") == agent_id]
        
        res = {
            "agent": agent,
            "recent_events": agent_events[:10]
        }
        return {"content": [{"type": "text", "text": json.dumps(res, indent=2)}]}
        
    elif name == "query_topology":
        group_by = arguments.get("group_by", "cluster")
        data = api_get(f"/api/topology?group_by={group_by}")
        return {"content": [{"type": "text", "text": json.dumps(data, indent=2)}]}
        
    elif name == "list_remediations":
        data = api_get("/api/remediations")
        return {"content": [{"type": "text", "text": json.dumps(data, indent=2)}]}
        
    elif name == "approve_remediation":
        rem_id = arguments.get("remediation_id")
        if not rem_id:
            return {"content": [{"type": "text", "text": "Error: remediation_id parameter is required"}]}
        data = api_post(f"/api/remediations/{rem_id}/approve")
        return {"content": [{"type": "text", "text": json.dumps(data, indent=2)}]}
        
    else:
        return {"content": [{"type": "text", "text": f"Error: unknown tool '{name}'"}]}

def main():
    # Set stdin/stdout to UTF-8
    sys.stdin.reconfigure(encoding='utf-8')
    sys.stdout.reconfigure(encoding='utf-8')
    
    while True:
        line = sys.stdin.readline()
        if not line:
            break
        
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue
        
        req_id = req.get("id")
        method = req.get("method")
        
        # Handle JSON-RPC request/response
        if method == "initialize":
            res = {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "ravn-mcp",
                        "version": "0.1.0"
                    }
                }
            }
            sys.stdout.write(json.dumps(res) + "\n")
            sys.stdout.flush()
            
        elif method == "notifications/initialized":
            # Notifications do not expect a response
            continue
            
        elif method == "tools/list":
            res = {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": handle_list_tools()
            }
            sys.stdout.write(json.dumps(res) + "\n")
            sys.stdout.flush()
            
        elif method == "tools/call":
            params = req.get("params", {})
            tool_name = params.get("name")
            args = params.get("arguments", {})
            
            res = {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": handle_call_tool(tool_name, args)
            }
            sys.stdout.write(json.dumps(res) + "\n")
            sys.stdout.flush()
            
        elif method:
            # Unsupported method
            res = {
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32601,
                    "message": f"Method '{method}' not found"
                }
            }
            sys.stdout.write(json.dumps(res) + "\n")
            sys.stdout.flush()

if __name__ == '__main__':
    main()
