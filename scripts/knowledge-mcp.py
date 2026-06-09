#!/usr/bin/env python3
import json
import os
import sys
import urllib.request
import urllib.parse
import re

# Configurations
BACKSTAGE_URL = os.environ.get("BACKSTAGE_URL", "")
BACKSTAGE_TOKEN = os.environ.get("BACKSTAGE_TOKEN", "")
CONFLUENCE_URL = os.environ.get("CONFLUENCE_URL", "")
CONFLUENCE_USER = os.environ.get("CONFLUENCE_USER", "")
CONFLUENCE_TOKEN = os.environ.get("CONFLUENCE_TOKEN", "")
NOTION_API_KEY = os.environ.get("NOTION_API_KEY", "")
LOCAL_RUNBOOKS_DIR = os.environ.get("LOCAL_RUNBOOKS_DIR", "/home/olafkfreund/Source/GitHub/ravn-agents/runbooks")
GITHUB_WIKI_DIR = os.environ.get("GITHUB_WIKI_DIR", "/home/olafkfreund/Source/GitHub/ravn-agents/wiki")

def load_dynamic_settings():
    global BACKSTAGE_URL, BACKSTAGE_TOKEN, CONFLUENCE_URL, CONFLUENCE_USER, CONFLUENCE_TOKEN
    global NOTION_API_KEY, LOCAL_RUNBOOKS_DIR, GITHUB_WIKI_DIR
    
    # Try fetching from ravn-server API first
    api_url = os.environ.get("RAVN_API_URL", "http://127.0.0.1:18080")
    settings_data = None
    
    try:
        url = f"{api_url.rstrip('/')}/api/settings"
        req = urllib.request.Request(url, method="GET")
        req.add_header("Accept", "application/json")
        with urllib.request.urlopen(req, timeout=1.0) as resp:
            settings_data = json.loads(resp.read().decode('utf-8'))
    except Exception:
        # Fall back to reading settings.json directly
        possible_paths = [
            "settings.json",
            "../settings.json",
            os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "settings.json")
        ]
        for path in possible_paths:
            if os.path.exists(path):
                try:
                    with open(path, "r", encoding="utf-8") as f:
                        settings_data = json.load(f)
                    break
                except Exception:
                    pass

    if settings_data and isinstance(settings_data, dict):
        knowledge = settings_data.get("knowledge")
        if isinstance(knowledge, dict):
            # Update only if configured in settings; fallback to current value if not set.
            # If enabled in settings, use settings. Otherwise if disabled, set to empty to override.
            if knowledge.get("backstage_enabled"):
                BACKSTAGE_URL = knowledge.get("backstage_url", BACKSTAGE_URL)
                BACKSTAGE_TOKEN = knowledge.get("backstage_token", BACKSTAGE_TOKEN)
            else:
                BACKSTAGE_URL = ""
                BACKSTAGE_TOKEN = ""
                
            if knowledge.get("confluence_enabled"):
                CONFLUENCE_URL = knowledge.get("confluence_url", CONFLUENCE_URL)
                CONFLUENCE_USER = knowledge.get("confluence_user", CONFLUENCE_USER)
                CONFLUENCE_TOKEN = knowledge.get("confluence_token", CONFLUENCE_TOKEN)
            else:
                CONFLUENCE_URL = ""
                CONFLUENCE_USER = ""
                CONFLUENCE_TOKEN = ""
                
            if knowledge.get("notion_enabled"):
                NOTION_API_KEY = knowledge.get("notion_api_key", NOTION_API_KEY)
            else:
                NOTION_API_KEY = ""
                
            LOCAL_RUNBOOKS_DIR = knowledge.get("local_runbooks_dir") or LOCAL_RUNBOOKS_DIR
            GITHUB_WIKI_DIR = knowledge.get("github_wiki_dir") or GITHUB_WIKI_DIR

# Initial load on startup
load_dynamic_settings()

# Built-in Mock Runbooks for Demo/Testing workloads
MOCK_RUNBOOKS = {
    "frontend": {
        "title": "Frontend Service Runbook",
        "owner": "team-frontend (on-call: @alice)",
        "source": "Backstage TechDocs",
        "content": """# Frontend Service Troubleshooting Guide
## Common Issues
### 1. Database Unreachable / Connection Refused
* **Symptoms:** Container logs show 'Database unreachable!' or HTTP 502/504 errors.
* **Diagnostics:**
  1. Check if the database service is running: `kubectl get pods -n ravn-test -l app=ravn-db`
  2. If the db pod is Terminating or Missing, wait for it to restart.
  3. If it is running, check if it responds on port 8080.
* **Remediation:** If the db pod is dead, recreate it. Once the database is online, restart the frontend pod to re-establish connections: `kubectl delete pod -n ravn-test -l app=ravn-frontend`."""
    },
    "db": {
        "title": "Database Component Specifications",
        "owner": "team-data (on-call: @bob)",
        "source": "Confluence Cloud",
        "content": """# Database (ravn-db) Administration
## Architecture
The database is a simple mock netcat server responding with HTTP responses on port 8080. It does not persist state.
## Troubleshooting
If the service stops responding, execute a restart. Because it has no persistence, it can be deleted and recreated at any time without data loss."""
    },
    "crasher": {
        "title": "Crasher Debugging Guide",
        "owner": "team-platform (on-call: @charlie)",
        "source": "Notion Workspace",
        "content": """# Crasher Pod Fault Analysis
* **Description:** The crasher pod is designed to fail periodically to test Ravn event logging and metrics.
* **Known Behavior:** It runs for 2 seconds, prints 'crashing', and exits with status 1.
* **Action:** This is an intentional crash loop. Do not trigger alert escalations."""
    },
    "flaky.service": {
        "title": "Host Flaky Service Recovery",
        "owner": "team-platform (on-call: @platform-oncall)",
        "source": "GitHub runbooks repo",
        "content": """# flaky.service Runbook
* **Description:** A systemd unit running on our NixOS development host. It occasionally fails to simulate host-level issues.
* **Remediation:** Run `systemctl restart flaky.service` to heal the unit."""
    },
    "fwupd-refresh.service": {
        "title": "Fwupd Refresh Service Hardware Fault",
        "owner": "team-platform",
        "source": "Systemd Wiki",
        "content": """# fwupd-refresh.service Issues
* **Symptoms:** Service fails to start with exit code 1.
* **Cause:** Often triggered on virtualized hosts (like VMs or containers) that lack physical firmware update capabilities.
* **Resolution:** Safe to ignore or disable on development/VM environments. Restarting it will fail persistently if no firmware update device is connected."""
    }
}

def search_local_markdown_dirs(query):
    results = []
    # Core directories to search recursively:
    # 1. Runbooks directory
    # 2. Docs folder (GitHub Pages source)
    # 3. docs-techdocs folder (Backstage TechDocs source)
    # 4. wiki folder (GitHub Wiki local checkout)
    search_paths = [
        LOCAL_RUNBOOKS_DIR,
        "/home/olafkfreund/Source/GitHub/ravn-agents/docs",
        "/home/olafkfreund/Source/GitHub/ravn-agents/docs-techdocs",
        GITHUB_WIKI_DIR
    ]
    
    for folder in search_paths:
        if not os.path.exists(folder):
            continue
        try:
            for root, _, files in os.walk(folder):
                for file in files:
                    if file.endswith(".md"):
                        filepath = os.path.join(root, file)
                        with open(filepath, "r", encoding="utf-8", errors="ignore") as f:
                            content = f.read()
                        if query.lower() in content.lower() or query.lower() in file.lower():
                            # Extract title or first line
                            first_line = content.split("\n")[0] if content else ""
                            title = first_line.lstrip("# ").strip() or file
                            
                            # Set appropriate source label
                            folder_basename = os.path.basename(folder.rstrip("/"))
                            if folder_basename == "docs":
                                source_label = f"GitHub Pages ({file})"
                            elif folder_basename == "docs-techdocs":
                                source_label = f"Backstage TechDocs ({file})"
                            elif folder_basename == "wiki":
                                source_label = f"GitHub Wiki ({file})"
                            else:
                                source_label = f"Local Runbook ({file})"
                                
                            results.append({
                                "title": title,
                                "owner": "repository",
                                "source": source_label,
                                "content": content
                            })
        except Exception as e:
            sys.stderr.write(f"[ERROR] Error searching folder {folder}: {e}\n")
    return results

def search_backstage_catalog(query):
    if not BACKSTAGE_URL:
        return []
    
    try:
        url = f"{BACKSTAGE_URL.rstrip('/')}/api/catalog/entities?filter=kind=component"
        req = urllib.request.Request(url, method="GET")
        if BACKSTAGE_TOKEN:
            req.add_header("Authorization", f"Bearer {BACKSTAGE_TOKEN}")
        req.add_header("Accept", "application/json")
        
        with urllib.request.urlopen(req, timeout=3) as resp:
            data = json.loads(resp.read().decode('utf-8'))
            results = []
            for item in data:
                metadata = item.get("metadata", {})
                spec = item.get("spec", {})
                name = metadata.get("name", "")
                description = metadata.get("description", "")
                
                if query.lower() in name.lower() or query.lower() in description.lower():
                    techdocs = metadata.get("annotations", {}).get("backstage.io/techdocs-ref", "None")
                    results.append({
                        "title": f"Backstage Catalog: {name}",
                        "owner": f"{spec.get('owner', 'unknown')}",
                        "source": "Backstage Software Catalog",
                        "content": f"Component: {name}\nDescription: {description}\nOwner: {spec.get('owner')}\nLifecycle: {spec.get('lifecycle', 'unknown')}\nTechDocs Reference: {techdocs}"
                    })
            return results
    except Exception as e:
        sys.stderr.write(f"[WARN] Backstage Catalog query failed: {e}\n")
        return []

def search_confluence(query):
    if not CONFLUENCE_URL or not CONFLUENCE_USER or not CONFLUENCE_TOKEN:
        return []
    
    try:
        import base64
        auth_str = f"{CONFLUENCE_USER}:{CONFLUENCE_TOKEN}"
        auth_bytes = auth_str.encode("utf-8")
        auth_b64 = base64.b64encode(auth_bytes).decode("utf-8")
        
        cql = f'text ~ "{query}"'
        url = f"{CONFLUENCE_URL.rstrip('/')}/rest/api/content/search?cql={urllib.parse.quote(cql)}&limit=3"
        req = urllib.request.Request(url, method="GET")
        req.add_header("Authorization", f"Basic {auth_b64}")
        req.add_header("Accept", "application/json")
        
        with urllib.request.urlopen(req, timeout=3) as resp:
            data = json.loads(resp.read().decode('utf-8'))
            results = []
            for item in data.get("results", []):
                results.append({
                    "title": item.get("title", ""),
                    "owner": "confluence",
                    "source": "Confluence Cloud",
                    "content": f"Page Title: {item.get('title')}\nLink: {CONFLUENCE_URL}/wiki{item.get('_links', {}).get('webui')}"
                })
            return results
    except Exception as e:
        sys.stderr.write(f"[WARN] Confluence query failed: {e}\n")
        return []

def search_all_sources(query, service):
    results = []
    
    # 1. Match built-in mock runbooks (direct name matching)
    for key, rb in MOCK_RUNBOOKS.items():
        if (service and service.lower() in key.lower()) or (query and query.lower() in key.lower()) or (query and key.lower() in query.lower()):
            results.append(rb)
            
    # 2. Check local directories: Runbooks, GitHub Pages, TechDocs, GitHub Wiki
    if query:
        results.extend(search_local_markdown_dirs(query))
    if service and service != query:
        results.extend(search_local_markdown_dirs(service))
        
    # 3. Query Backstage Catalog (if configured)
    if query:
        results.extend(search_backstage_catalog(query))
        
    # 4. Query Confluence (if configured)
    if query:
        results.extend(search_confluence(query))
        
    # Deduplicate results by title
    seen = set()
    deduped = []
    for r in results:
        if r["title"] not in seen:
            seen.add(r["title"])
            deduped.append(r)
            
    return deduped

def handle_list_tools():
    return {
        "tools": [
            {
                "name": "search_runbooks",
                "description": "Federated search for service runbooks, diagnostic documents, Backstage TechDocs, and GitHub Wikis.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Keywords describing the failure (e.g., 'database connection refused')"
                        },
                        "service": {
                            "type": "string",
                            "description": "The name of the service that is failing (e.g., 'frontend', 'db')"
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "get_page_content",
                "description": "Retrieve the full text content of a runbook page.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "The exact title of the runbook/page"
                        }
                    },
                    "required": ["title"]
                }
            },
            {
                "name": "list_service_contacts",
                "description": "Get contact information and active on-call details for a service.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "service": {
                            "type": "string",
                            "description": "The name of the service"
                        }
                    },
                    "required": ["service"]
                }
            }
        ]
    }

def handle_call_tool(name, arguments):
    load_dynamic_settings()
    if name == "search_runbooks":
        query = arguments.get("query", "")
        service = arguments.get("service", "")
        results = search_all_sources(query, service)
        
        if not results:
            return {
                "content": [{
                    "type": "text",
                    "text": f"No runbooks found for query '{query}' (service: '{service}')."
                }]
            }
            
        output = []
        for r in results:
            output.append(f"### {r['title']} ({r['source']})")
            output.append(f"**Owner/On-call:** {r['owner']}")
            output.append(f"\n{r['content']}")
            output.append("\n" + "-"*40 + "\n")
            
        return {"content": [{"type": "text", "text": "\n".join(output)}]}
        
    elif name == "get_page_content":
        title = arguments.get("title", "")
        for rb in MOCK_RUNBOOKS.values():
            if rb["title"].lower() == title.lower():
                return {"content": [{"type": "text", "text": rb["content"]}]}
                
        local_results = search_local_markdown_dirs(title)
        if local_results:
            return {"content": [{"type": "text", "text": local_results[0]["content"]}]}
            
        return {"content": [{"type": "text", "text": f"Page '{title}' not found."}]}
        
    elif name == "list_service_contacts":
        service = arguments.get("service", "")
        for key, rb in MOCK_RUNBOOKS.items():
            if service.lower() in key.lower():
                return {
                    "content": [{
                        "type": "text",
                        "text": f"Service: {key}\nOwner: {rb['owner']}\nDocumentation Source: {rb['source']}"
                    }]
                }
                
        return {"content": [{"type": "text", "text": f"No contacts found for service '{service}'."}]}
        
    else:
        return {"content": [{"type": "text", "text": f"Error: unknown tool '{name}'"}]}

def main():
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
                        "name": "unified-knowledge-mcp",
                        "version": "0.2.0"
                    }
                }
            }
            sys.stdout.write(json.dumps(res) + "\n")
            sys.stdout.flush()
            
        elif method == "notifications/initialized":
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
