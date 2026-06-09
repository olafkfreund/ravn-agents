import { useState, useEffect } from "react";

interface McpServer {
  id: string;
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
}

export function Settings() {
  // Remediation Settings
  const [autoRemediation, setAutoRemediation] = useState(false);
  const [riskTier, setRiskTier] = useState("safe");

  // Inference Settings
  const [inferenceEnabled, setInferenceEnabled] = useState(true);
  const [inferenceEndpoint, setInferenceEndpoint] = useState("http://127.0.0.1:15000");
  const [inferenceModel, setInferenceModel] = useState("default");

  // Knowledge Connector Settings
  const [localRunbooksDir, setLocalRunbooksDir] = useState("");
  const [githubWikiDir, setGithubWikiDir] = useState("");
  const [backstageEnabled, setBackstageEnabled] = useState(false);
  const [backstageUrl, setBackstageUrl] = useState("");
  const [backstageToken, setBackstageToken] = useState("");
  const [confluenceEnabled, setConfluenceEnabled] = useState(false);
  const [confluenceUrl, setConfluenceUrl] = useState("");
  const [confluenceUser, setConfluenceUser] = useState("");
  const [confluenceToken, setConfluenceToken] = useState("");
  const [notionEnabled, setNotionEnabled] = useState(false);
  const [notionApiKey, setNotionApiKey] = useState("");

  // Third-Party MCP Servers
  const [mcpServers, setMcpServers] = useState<McpServer[]>(() => {
    const stored = localStorage.getItem("ravn_mcp_servers");
    if (stored) {
      try {
        return jsonParse(stored);
      } catch {
        return defaultMcpServers();
      }
    }
    return defaultMcpServers();
  });

  const [newServerName, setNewServerName] = useState("");
  const [newServerCmd, setNewServerCmd] = useState("");
  const [newServerArgs, setNewServerArgs] = useState("");
  const [showAddForm, setShowAddForm] = useState(false);
  const [savedMessage, setSavedMessage] = useState("");

  // Helper because JSON.parse types can be annoying in TS without wrapper
  function jsonParse(str: string): McpServer[] {
    return JSON.parse(str) as McpServer[];
  }

  // Fetch settings from API on mount
  useEffect(() => {
    fetch("/api/settings")
      .then((res) => {
        if (!res.ok) throw new Error("Backend settings not found");
        return res.json();
      })
      .then((data) => {
        setAutoRemediation(data.auto_remediation ?? false);
        setRiskTier(data.risk_tier ?? "safe");
        setInferenceEnabled(data.inference_enabled ?? true);
        setInferenceEndpoint(data.inference_endpoint ?? "http://127.0.0.1:15000");
        setInferenceModel(data.inference_model ?? "default");

        const k = data.knowledge;
        if (k) {
          setLocalRunbooksDir(k.local_runbooks_dir ?? "");
          setGithubWikiDir(k.github_wiki_dir ?? "");
          setBackstageEnabled(k.backstage_enabled ?? false);
          setBackstageUrl(k.backstage_url ?? "");
          setBackstageToken(k.backstage_token ?? "");
          setConfluenceEnabled(k.confluence_enabled ?? false);
          setConfluenceUrl(k.confluence_url ?? "");
          setConfluenceUser(k.confluence_user ?? "");
          setConfluenceToken(k.confluence_token ?? "");
          setNotionEnabled(k.notion_enabled ?? false);
          setNotionApiKey(k.notion_api_key ?? "");
        }
      })
      .catch((err) => console.error("Error fetching settings:", err));
  }, []);

  // Save third-party MCP servers to localStorage when they change
  useEffect(() => {
    localStorage.setItem("ravn_mcp_servers", JSON.stringify(mcpServers));
  }, [mcpServers]);

  function defaultMcpServers(): McpServer[] {
    return [
      {
        id: "gke-mcp",
        name: "Google Cloud GKE MCP",
        command: "npx",
        args: ["-y", "@google/gke-mcp-server"],
        enabled: true,
      },
      {
        id: "k8s-mcp",
        name: "Kubernetes Resource MCP",
        command: "npx",
        args: ["-y", "@modelcontextprotocol/server-kubernetes"],
        enabled: false,
      }
    ];
  }

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    const payload = {
      auto_remediation: autoRemediation,
      risk_tier: riskTier,
      inference_enabled: inferenceEnabled,
      inference_endpoint: inferenceEndpoint,
      inference_model: inferenceModel,
      knowledge: {
        local_runbooks_dir: localRunbooksDir,
        github_wiki_dir: githubWikiDir,
        backstage_enabled: backstageEnabled,
        backstage_url: backstageUrl,
        backstage_token: backstageToken,
        confluence_enabled: confluenceEnabled,
        confluence_url: confluenceUrl,
        confluence_user: confluenceUser,
        confluence_token: confluenceToken,
        notion_enabled: notionEnabled,
        notion_api_key: notionApiKey,
      }
    };

    try {
      const res = await fetch("/api/settings", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (res.ok) {
        setSavedMessage("Configuration saved successfully ✓");
      } else {
        setSavedMessage("Failed to save configuration ✗");
      }
    } catch (err) {
      setSavedMessage("Error saving configuration ✗");
    }
    setTimeout(() => setSavedMessage(""), 3000);
  };

  const handleAddMcpServer = (e: React.FormEvent) => {
    e.preventDefault();
    if (!newServerName || !newServerCmd) return;
    const parsedArgs = newServerArgs
      .split(" ")
      .map((a) => a.trim())
      .filter((a) => a.length > 0);
    const newServer: McpServer = {
      id: Math.random().toString(36).substr(2, 9),
      name: newServerName,
      command: newServerCmd,
      args: parsedArgs,
      enabled: true,
    };
    setMcpServers([...mcpServers, newServer]);
    setNewServerName("");
    setNewServerCmd("");
    setNewServerArgs("");
    setShowAddForm(false);
  };

  const toggleMcpServer = (id: string) => {
    setMcpServers(
      mcpServers.map((s) => (s.id === id ? { ...s, enabled: !s.enabled } : s))
    );
  };

  const deleteMcpServer = (id: string) => {
    setMcpServers(mcpServers.filter((s) => s.id !== id));
  };

  // Get project root path for MCP config display
  const mcpConfigCode = JSON.stringify(
    {
      mcpServers: {
        "ravn-mcp": {
          command: "python3",
          args: ["/home/olafkfreund/Source/GitHub/ravn-agents/scripts/ravn_mcp_server.py"],
          env: {
            RAVN_API_URL: "http://127.0.0.1:18080"
          }
        }
      }
    },
    null,
    2
  );

  return (
    <div className="space-y-6 max-w-4xl pb-12">
      <div>
        <h2 className="font-display text-2xl font-bold tracking-tight">System Settings</h2>
        <p className="text-sm text-fg-mute">Configure agent self-healing policy, inference endpoints, and Model Context Protocol (MCP) integrations.</p>
      </div>

      <form onSubmit={handleSave} className="space-y-6">
        {/* Auto-Remediation Panel */}
        <div className="rounded-xl border border-line bg-surface p-5 shadow-card space-y-4">
          <div className="flex items-start justify-between">
            <div className="space-y-1">
              <h3 className="font-display text-lg font-bold">Auto-Remediation & Self-Healing</h3>
              <p className="text-xs text-fg-mute">Control if Ravn can automatically apply approved templates in response to failures.</p>
            </div>
            <label className="relative inline-flex items-center cursor-pointer select-none">
              <input
                type="checkbox"
                checked={autoRemediation}
                onChange={(e) => setAutoRemediation(e.target.checked)}
                className="sr-only peer"
              />
              <div className="w-11 h-6 bg-surface-2 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-fg-mute after:border-line after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-accent peer-checked:after:bg-surface"></div>
            </label>
          </div>

          <div className={`space-y-3 transition-opacity duration-200 ${autoRemediation ? "opacity-100" : "opacity-50 pointer-events-none"}`}>
            <label className="block text-xs font-mono uppercase tracking-[0.14em] text-fg-mute">
              Maximum Risk Level for Automation
            </label>
            <div className="grid grid-cols-3 gap-3">
              {(["safe", "guarded", "dangerous"] as const).map((tier) => (
                <button
                  type="button"
                  key={tier}
                  onClick={() => setRiskTier(tier)}
                  className={`flex flex-col rounded-lg border p-3 text-left transition-colors ${
                    riskTier === tier
                      ? "border-accent bg-accent/5"
                      : "border-line bg-surface-2 hover:border-fg-mute"
                  }`}
                >
                  <span className="font-mono text-xs uppercase font-bold text-fg">{tier}</span>
                  <span className="mt-1 text-[10px] text-fg-mute leading-normal">
                    {tier === "safe" && "Restart stalled units, safe read actions. No data loss risk."}
                    {tier === "guarded" && "Reversible actions like NixOS rollbacks or service restarts."}
                    {tier === "dangerous" && "Actions with potential data loss or high blast radius. Never auto-executes."}
                  </span>
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* Inference Configuration */}
        <div className="rounded-xl border border-line bg-surface p-5 shadow-card space-y-4">
          <div className="flex items-start justify-between">
            <div className="space-y-1">
              <h3 className="font-display text-lg font-bold">Inference Engine (Async Explanations)</h3>
              <p className="text-xs text-fg-mute">Connect to an LLM provider to explain K8s failures and suggest commands.</p>
            </div>
            <label className="relative inline-flex items-center cursor-pointer select-none">
              <input
                type="checkbox"
                checked={inferenceEnabled}
                onChange={(e) => setInferenceEnabled(e.target.checked)}
                className="sr-only peer"
              />
              <div className="w-11 h-6 bg-surface-2 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-fg-mute after:border-line after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-accent peer-checked:after:bg-surface"></div>
            </label>
          </div>

          <div className={`grid grid-cols-2 gap-4 transition-opacity duration-200 ${inferenceEnabled ? "opacity-100" : "opacity-50 pointer-events-none"}`}>
            <div>
              <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">
                OpenAI-Compatible Endpoint
              </label>
              <input
                type="url"
                value={inferenceEndpoint}
                onChange={(e) => setInferenceEndpoint(e.target.value)}
                placeholder="http://127.0.0.1:15000"
                className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
              />
            </div>
            <div>
              <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">
                Model Name
              </label>
              <input
                type="text"
                value={inferenceModel}
                onChange={(e) => setInferenceModel(e.target.value)}
                placeholder="default"
                className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
              />
            </div>
          </div>
        </div>

        {/* Documentation & Wiki Sources */}
        <div className="rounded-xl border border-line bg-surface p-5 shadow-card space-y-5">
          <div className="space-y-1">
            <h3 className="font-display text-lg font-bold">Documentation & Wiki Sources</h3>
            <p className="text-xs text-fg-mute">Configure file paths and credentials for Backstage, Confluence, Notion, and local runbooks.</p>
          </div>

          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">
                Local Runbooks Directory
              </label>
              <input
                type="text"
                value={localRunbooksDir}
                onChange={(e) => setLocalRunbooksDir(e.target.value)}
                className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
                placeholder="/home/olafkfreund/Source/GitHub/ravn-agents/runbooks"
              />
            </div>
            <div>
              <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">
                Local GitHub Wiki Directory
              </label>
              <input
                type="text"
                value={githubWikiDir}
                onChange={(e) => setGithubWikiDir(e.target.value)}
                className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
                placeholder="/home/olafkfreund/Source/GitHub/ravn-agents/wiki"
              />
            </div>
          </div>

          <hr className="border-line-soft" />

          {/* Backstage Integration */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <span className="font-semibold text-sm">Backstage Software Catalog</span>
              <label className="relative inline-flex items-center cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={backstageEnabled}
                  onChange={(e) => setBackstageEnabled(e.target.checked)}
                  className="sr-only peer"
                />
                <div className="w-9 h-5 bg-surface-2 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-fg-mute after:border-line after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-accent peer-checked:after:bg-surface"></div>
              </label>
            </div>
            {backstageEnabled && (
              <div className="grid grid-cols-2 gap-4 pl-4 border-l-2 border-line-soft">
                <div>
                  <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">Backstage URL</label>
                  <input
                    type="url"
                    value={backstageUrl}
                    onChange={(e) => setBackstageUrl(e.target.value)}
                    className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
                    placeholder="http://backstage.internal"
                  />
                </div>
                <div>
                  <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">API Token (Optional)</label>
                  <input
                    type="password"
                    value={backstageToken}
                    onChange={(e) => setBackstageToken(e.target.value)}
                    className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
                    placeholder="Bearer token"
                  />
                </div>
              </div>
            )}
          </div>

          <hr className="border-line-soft" />

          {/* Confluence Integration */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <span className="font-semibold text-sm">Confluence Cloud Spaces</span>
              <label className="relative inline-flex items-center cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={confluenceEnabled}
                  onChange={(e) => setConfluenceEnabled(e.target.checked)}
                  className="sr-only peer"
                />
                <div className="w-9 h-5 bg-surface-2 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-fg-mute after:border-line after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-accent peer-checked:after:bg-surface"></div>
              </label>
            </div>
            {confluenceEnabled && (
              <div className="grid grid-cols-3 gap-4 pl-4 border-l-2 border-line-soft">
                <div>
                  <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">Confluence Site URL</label>
                  <input
                    type="url"
                    value={confluenceUrl}
                    onChange={(e) => setConfluenceUrl(e.target.value)}
                    className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
                    placeholder="https://org.atlassian.net/wiki"
                  />
                </div>
                <div>
                  <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">Username / Email</label>
                  <input
                    type="email"
                    value={confluenceUser}
                    onChange={(e) => setConfluenceUser(e.target.value)}
                    className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
                    placeholder="email@company.com"
                  />
                </div>
                <div>
                  <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">API Token</label>
                  <input
                    type="password"
                    value={confluenceToken}
                    onChange={(e) => setConfluenceToken(e.target.value)}
                    className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
                    placeholder="Atlassian API token"
                  />
                </div>
              </div>
            )}
          </div>

          <hr className="border-line-soft" />

          {/* Notion Integration */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <span className="font-semibold text-sm">Notion Workspace</span>
              <label className="relative inline-flex items-center cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={notionEnabled}
                  onChange={(e) => setNotionEnabled(e.target.checked)}
                  className="sr-only peer"
                />
                <div className="w-9 h-5 bg-surface-2 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-fg-mute after:border-line after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-accent peer-checked:after:bg-surface"></div>
              </label>
            </div>
            {notionEnabled && (
              <div className="grid grid-cols-2 gap-4 pl-4 border-l-2 border-line-soft">
                <div className="col-span-2">
                  <label className="block text-[10px] font-mono uppercase tracking-[0.14em] text-fg-mute mb-1">Notion API Key</label>
                  <input
                    type="password"
                    value={notionApiKey}
                    onChange={(e) => setNotionApiKey(e.target.value)}
                    className="w-full rounded-md border border-line bg-bg px-3 py-1.5 font-mono text-xs text-fg focus:border-accent focus-ring"
                    placeholder="secret_notion..."
                  />
                </div>
              </div>
            )}
          </div>
        </div>

        {/* Action Bar */}
        <div className="flex items-center justify-between gap-4">
          {savedMessage && <span className="font-mono text-xs text-accent font-semibold">{savedMessage}</span>}
          <button
            type="submit"
            className="ml-auto rounded-lg bg-accent px-5 py-2 font-display text-sm font-bold text-surface hover:bg-accent-2 transition-colors focus-ring"
          >
            Save Configuration
          </button>
        </div>
      </form>

      {/* Ravn's Built-in MCP Server */}
      <div className="rounded-xl border border-line bg-surface p-5 shadow-card space-y-4">
        <div className="space-y-1">
          <div className="flex items-center gap-2">
            <h3 className="font-display text-lg font-bold">Ravn Model Context Protocol (MCP) Server</h3>
            <span className="rounded bg-accent/10 border border-accent/20 px-1.5 py-0.5 font-mono text-[9px] font-bold text-accent">Active</span>
          </div>
          <p className="text-xs text-fg-mute">Expose Ravn fleet telemetry, event logs, and remediations directly to external LLM clients (like Gemini Desktop or Claude CLI).</p>
        </div>

        <div className="space-y-3 font-sans text-xs leading-relaxed text-fg-dim">
          <p>
            Ravn includes a built-in MCP server that works over stdin/stdout, allowing client applications to query agent status, lookup event details, and approve remediation actions directly from your chat prompt.
          </p>
          <p className="font-semibold text-fg">Client Configuration JSON:</p>
          <pre className="p-3 bg-bg border border-line rounded-lg font-mono text-[10px] text-fg-dim overflow-x-auto select-all">
            {mcpConfigCode}
          </pre>
        </div>
      </div>

      {/* Third-Party MCP Servers Integration */}
      <div className="rounded-xl border border-line bg-surface p-5 shadow-card space-y-4">
        <div className="flex items-center justify-between">
          <div className="space-y-1">
            <h3 className="font-display text-lg font-bold">Infrastructure Resource MCP Lookups</h3>
            <p className="text-xs text-fg-mute">Integrate third-party MCP servers so the Ravn agent can query GKE, AWS, or local cluster resources for better diagnostics.</p>
          </div>
          <button
            type="button"
            onClick={() => setShowAddForm(!showAddForm)}
            className="rounded border border-line px-2.5 py-1 text-xs text-fg-dim hover:border-accent hover:text-fg"
          >
            {showAddForm ? "Cancel" : "+ Add MCP Server"}
          </button>
        </div>

        {showAddForm && (
          <form onSubmit={handleAddMcpServer} className="p-4 bg-bg rounded-lg border border-line-soft space-y-3">
            <h4 className="font-display text-xs font-bold uppercase tracking-wider text-fg-mute">New MCP integration</h4>
            <div className="grid grid-cols-3 gap-3">
              <input
                value={newServerName}
                onChange={(e) => setNewServerName(e.target.value)}
                placeholder="e.g. AWS EKS MCP"
                className="rounded border border-line bg-surface px-2.5 py-1 text-xs text-fg focus:border-accent focus-ring"
                required
              />
              <input
                value={newServerCmd}
                onChange={(e) => setNewServerCmd(e.target.value)}
                placeholder="command (e.g. npx)"
                className="rounded border border-line bg-surface px-2.5 py-1 text-xs text-fg focus:border-accent focus-ring"
                required
              />
              <input
                value={newServerArgs}
                onChange={(e) => setNewServerArgs(e.target.value)}
                placeholder="args (space separated)"
                className="rounded border border-line bg-surface px-2.5 py-1 text-xs text-fg focus:border-accent focus-ring"
              />
            </div>
            <button
              type="submit"
              className="rounded bg-accent px-3 py-1 text-xs font-semibold text-surface hover:bg-accent-2"
            >
              Add Server
            </button>
          </form>
        )}

        <div className="space-y-2">
          {mcpServers.map((server) => (
            <div key={server.id} className="flex items-center justify-between p-3 bg-bg border border-line rounded-lg">
              <div className="space-y-0.5">
                <div className="flex items-center gap-2">
                  <span className="font-semibold text-xs text-fg">{server.name}</span>
                  <span className={`h-1.5 w-1.5 rounded-full ${server.enabled ? "bg-sev-notice" : "bg-fg-mute"}`} />
                </div>
                <div className="font-mono text-[9px] text-fg-mute">
                  {server.command} {server.args.join(" ")}
                </div>
              </div>
              <div className="flex items-center gap-3">
                <button
                  type="button"
                  onClick={() => toggleMcpServer(server.id)}
                  className={`rounded px-2.5 py-1 text-[11px] font-semibold border ${
                    server.enabled
                      ? "border-accent bg-accent/5 text-accent hover:bg-accent/15"
                      : "border-line text-fg-mute hover:border-fg-dim hover:text-fg"
                  }`}
                >
                  {server.enabled ? "Enabled" : "Disabled"}
                </button>
                <button
                  type="button"
                  onClick={() => deleteMcpServer(server.id)}
                  className="text-fg-mute hover:text-sev-error text-xs"
                >
                  Delete
                </button>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
