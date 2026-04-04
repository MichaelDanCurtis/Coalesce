import { useState, useEffect, useCallback } from "react";
import { api } from "../api/client";

interface McpServer {
  id: string;
  name: string;
  transport: string;
  status: string;
  tools: Array<{ name: string; description?: string }>;
  enabled: boolean;
  source: string;
  command?: string;
  url?: string;
  last_connected?: number;
  error?: string;
}

export default function McpPanel() {
  const [servers, setServers] = useState<McpServer[]>([]);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [showAdd, setShowAdd] = useState(false);

  const fetchServers = useCallback(async () => {
    try {
      const data = await api.getMcpServers();
      setServers((data as any).servers || []);
    } catch {
      setServers([]);
    }
    setLoading(false);
  }, []);

  useEffect(() => { fetchServers(); }, [fetchServers]);

  const handleScan = async () => {
    setScanning(true);
    try {
      const data = await api.scanMcpServers();
      setServers((data as any).servers || []);
    } catch {}
    setScanning(false);
  };

  const handleToggle = async (id: string) => {
    try {
      await api.toggleMcpServer(id);
      await fetchServers();
    } catch {}
  };

  const handleRemove = async (id: string) => {
    try {
      await api.removeMcpServer(id);
      await fetchServers();
    } catch {}
  };

  const handleAdd = async (server: { name: string; transport: string; command?: string; args?: string[]; url?: string }) => {
    try {
      await api.registerMcpServer(server);
      await fetchServers();
      setShowAdd(false);
    } catch {}
  };

  const transportColor = (t: string) => {
    switch (t.toLowerCase()) {
      case "stdio": return "bg-blue-500/20 text-blue-400";
      case "sse": return "bg-purple-500/20 text-purple-400";
      case "streamablehttp": return "bg-cyan-500/20 text-cyan-400";
      case "websocket": return "bg-amber-500/20 text-amber-400";
      case "grpc": return "bg-emerald-500/20 text-emerald-400";
      default: return "bg-tertiary text-secondary";
    }
  };

  const statusDot = (s: string) => {
    switch (s.toLowerCase()) {
      case "connected": return "bg-emerald-400";
      case "error": return "bg-red-400";
      case "starting": return "bg-amber-400 animate-pulse";
      default: return "bg-gray-400";
    }
  };

  const sourceLabel = (s: string) => {
    switch (s.toLowerCase()) {
      case "claudecode": return "Claude Code";
      case "cursor": return "Cursor";
      case "vscode": return "VS Code";
      case "windsurf": return "Windsurf";
      default: return "Manual";
    }
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold">MCP Servers</h2>
          <p className="text-sm text-secondary mt-1">
            Model Context Protocol servers provide tools to LLM providers
          </p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={() => setShowAdd(!showAdd)}
            className="rounded-lg border border-themed px-4 py-2 text-sm hover:border-brand-500/50 transition-colors"
          >
            {showAdd ? "Cancel" : "+ Add Server"}
          </button>
          <button
            onClick={handleScan}
            disabled={scanning}
            className="rounded-lg bg-brand-500 px-4 py-2 text-sm font-medium text-white hover:bg-brand-600 transition-colors disabled:opacity-50"
          >
            {scanning ? "Scanning..." : "Scan for Servers"}
          </button>
        </div>
      </div>

      {showAdd && <AddServerForm onSubmit={handleAdd} onCancel={() => setShowAdd(false)} />}

      {/* Stats bar */}
      <div className="grid grid-cols-4 gap-3">
        <div className="card text-center">
          <p className="text-2xl font-bold">{servers.length}</p>
          <p className="text-xs text-secondary">Total Servers</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold text-emerald-400">
            {servers.filter(s => s.status === "connected").length}
          </p>
          <p className="text-xs text-secondary">Connected</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold text-brand-400">
            {servers.reduce((sum, s) => sum + s.tools.length, 0)}
          </p>
          <p className="text-xs text-secondary">Total Tools</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold">
            {new Set(servers.map(s => s.transport)).size}
          </p>
          <p className="text-xs text-secondary">Transports</p>
        </div>
      </div>

      {loading ? (
        <div className="card text-center py-8">
          <p className="text-sm text-secondary">Loading MCP servers...</p>
        </div>
      ) : servers.length === 0 ? (
        <div className="card text-center py-12">
          <div className="text-4xl mb-3 opacity-30">MCP</div>
          <p className="text-sm text-secondary mb-2">No MCP servers found</p>
          <p className="text-xs text-secondary mb-4">
            Click "Scan for Servers" to detect MCP configurations from Claude Code, Cursor, and VS Code.
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          {servers.map((server) => (
            <div
              key={server.id}
              className="card cursor-pointer hover:border-brand-500/30 transition-colors"
              onClick={() => setExpanded(expanded === server.id ? null : server.id)}
            >
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-3">
                  <span className={`h-2.5 w-2.5 rounded-full ${statusDot(server.status)}`} />
                  <div>
                    <div className="flex items-center gap-2">
                      <p className="text-sm font-medium">{server.name}</p>
                      <span className={`rounded-full px-2 py-0.5 text-[10px] font-medium ${transportColor(server.transport)}`}>
                        {server.transport}
                      </span>
                      <span className="rounded-full bg-tertiary px-2 py-0.5 text-[10px] text-secondary">
                        {sourceLabel(server.source)}
                      </span>
                    </div>
                    <p className="text-xs text-secondary mt-0.5">
                      {server.tools.length} tool{server.tools.length !== 1 ? "s" : ""}
                      {server.command && ` · ${server.command}`}
                      {server.url && ` · ${server.url}`}
                    </p>
                  </div>
                </div>
                <div className="flex items-center gap-3">
                  {server.error && (
                    <span className="text-[10px] text-red-400 max-w-[200px] truncate">
                      {server.error}
                    </span>
                  )}
                  <button
                    onClick={(e) => { e.stopPropagation(); handleToggle(server.id); }}
                    className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
                      server.enabled ? "bg-brand-500" : "bg-tertiary"
                    }`}
                  >
                    <span className={`inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform ${
                      server.enabled ? "translate-x-4.5" : "translate-x-0.5"
                    }`} />
                  </button>
                  <button
                    onClick={(e) => { e.stopPropagation(); handleRemove(server.id); }}
                    className="text-xs text-secondary hover:text-red-400 transition-colors"
                  >
                    Del
                  </button>
                </div>
              </div>

              {expanded === server.id && server.tools.length > 0 && (
                <div className="mt-3 border-t border-themed pt-3">
                  <p className="text-xs text-secondary mb-2">Available Tools</p>
                  <div className="grid grid-cols-2 gap-2">
                    {server.tools.map((tool) => (
                      <div key={tool.name} className="rounded-lg bg-tertiary/50 px-3 py-2">
                        <p className="text-xs font-medium">{tool.name}</p>
                        {tool.description && (
                          <p className="text-[10px] text-secondary mt-0.5 line-clamp-2">{tool.description}</p>
                        )}
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function AddServerForm({ onSubmit, onCancel }: {
  onSubmit: (s: { name: string; transport: string; command?: string; args?: string[]; url?: string }) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [transport, setTransport] = useState("stdio");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [url, setUrl] = useState("");

  const inputClass = "w-full rounded-lg border border-themed bg-primary px-2 py-1.5 text-xs focus:border-brand-500/50 focus:outline-none";
  const selectClass = inputClass;

  return (
    <div className="card border-brand-500/30 bg-brand-500/5 space-y-3">
      <h3 className="text-sm font-medium">Add MCP Server</h3>
      <input type="text" placeholder="Server name" value={name} onChange={e => setName(e.target.value)} className={inputClass} />
      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="text-xs text-secondary mb-1 block">Transport</label>
          <select value={transport} onChange={e => setTransport(e.target.value)} className={selectClass}>
            <option value="stdio">Stdio</option>
            <option value="sse">SSE</option>
            <option value="streamablehttp">Streamable HTTP</option>
            <option value="websocket">WebSocket</option>
          </select>
        </div>
        {transport === "stdio" ? (
          <div>
            <label className="text-xs text-secondary mb-1 block">Command</label>
            <input type="text" placeholder="e.g. npx" value={command} onChange={e => setCommand(e.target.value)} className={inputClass} />
          </div>
        ) : (
          <div>
            <label className="text-xs text-secondary mb-1 block">URL</label>
            <input type="text" placeholder="http://localhost:3000/sse" value={url} onChange={e => setUrl(e.target.value)} className={inputClass} />
          </div>
        )}
      </div>
      {transport === "stdio" && (
        <div>
          <label className="text-xs text-secondary mb-1 block">Arguments (space-separated)</label>
          <input type="text" placeholder="-y @modelcontextprotocol/server-filesystem /tmp" value={args} onChange={e => setArgs(e.target.value)} className={inputClass} />
        </div>
      )}
      <div className="flex gap-2">
        <button
          onClick={() => { if (!name.trim()) return; onSubmit({ name: name.trim(), transport, command: command || undefined, args: args ? args.split(/\s+/) : undefined, url: url || undefined }); }}
          className="rounded-lg bg-brand-500 px-3 py-1.5 text-xs text-white hover:bg-brand-600 transition-colors"
        >
          Add Server
        </button>
        <button onClick={onCancel} className="rounded-lg border border-themed px-3 py-1.5 text-xs hover:border-brand-500/50 transition-colors">
          Cancel
        </button>
      </div>
    </div>
  );
}
