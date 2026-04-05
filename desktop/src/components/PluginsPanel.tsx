import { useState, useEffect, useCallback } from "react";
import { api } from "../api/client";

interface Plugin {
  enabled: boolean;
  path: string;
  config: any;
}

export default function PluginsPanel() {
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);

  const fetchPlugins = useCallback(async () => {
    try {
      const data = await api.getPlugins();
      setPlugins((data as any).plugins || []);
    } catch {
      setPlugins([]);
    }
    setLoading(false);
  }, []);

  useEffect(() => { fetchPlugins(); }, [fetchPlugins]);

  const handleScan = async () => {
    setScanning(true);
    try {
      const data = await api.scanPlugins();
      setPlugins((data as any).plugins || []);
    } catch {}
    setScanning(false);
  };

  const handleToggle = async (plugin: Plugin) => {
    const name = plugin.path.split("/").pop() || plugin.path;
    try {
      await api.togglePlugin(name);
      await fetchPlugins();
    } catch {}
  };

  const enabledCount = plugins.filter(p => p.enabled).length;
  const wasmCount = plugins.filter(p => p.path.endsWith(".wasm")).length;

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold">Plugins</h2>
          <p className="text-sm text-secondary mt-1">
            WASM plugins extend routing, transforms, and provider behavior
          </p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={fetchPlugins}
            className="rounded-lg border border-themed px-4 py-2 text-sm hover:border-brand-500/50 transition-colors"
          >
            Refresh
          </button>
          <button
            onClick={handleScan}
            disabled={scanning}
            className="rounded-lg bg-brand-500 px-4 py-2 text-sm font-medium text-white hover:bg-brand-600 transition-colors disabled:opacity-50"
          >
            {scanning ? "Scanning..." : "Scan Plugins"}
          </button>
        </div>
      </div>

      {/* Stats bar */}
      <div className="grid grid-cols-3 gap-3">
        <div className="card text-center">
          <p className="text-2xl font-bold">{plugins.length}</p>
          <p className="text-xs text-secondary">Total Plugins</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold text-emerald-400">{enabledCount}</p>
          <p className="text-xs text-secondary">Enabled</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold text-brand-400">{wasmCount}</p>
          <p className="text-xs text-secondary">WASM Modules</p>
        </div>
      </div>

      {/* Plugin directory info */}
      <div className="card bg-brand-500/5 border-brand-500/20">
        <p className="text-xs text-secondary">
          Plugin directory: <span className="font-mono text-primary">~/.config/coalesce/plugins/</span>
        </p>
        <p className="text-[10px] text-secondary mt-1">
          Place .wasm plugin files in this directory and click "Scan Plugins" to discover them.
        </p>
      </div>

      {loading ? (
        <div className="card text-center py-8">
          <p className="text-sm text-secondary">Loading plugins...</p>
        </div>
      ) : plugins.length === 0 ? (
        <div className="card text-center py-12">
          <div className="text-4xl mb-3 opacity-30">PL</div>
          <p className="text-sm text-secondary mb-2">No plugins found</p>
          <p className="text-xs text-secondary mb-4">
            Add .wasm files to the plugins directory and click "Scan Plugins" to discover them.
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          {plugins.map((plugin, idx) => {
            const fileName = plugin.path.split("/").pop() || plugin.path;
            const isWasm = plugin.path.endsWith(".wasm");
            return (
              <div key={idx} className="card hover:border-brand-500/30 transition-colors">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <div>
                      <div className="flex items-center gap-2">
                        <p className="text-sm font-medium">{fileName}</p>
                        <span className={`rounded-full px-2 py-0.5 text-[10px] font-medium ${
                          isWasm ? "bg-purple-500/20 text-purple-400" : "bg-tertiary text-secondary"
                        }`}>
                          {isWasm ? "WASM" : "Native"}
                        </span>
                      </div>
                      <p className="text-xs text-secondary mt-0.5 font-mono">{plugin.path}</p>
                    </div>
                  </div>
                  <button
                    onClick={() => handleToggle(plugin)}
                    className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
                      plugin.enabled ? "bg-brand-500" : "bg-tertiary"
                    }`}
                  >
                    <span className={`inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform ${
                      plugin.enabled ? "translate-x-4.5" : "translate-x-0.5"
                    }`} />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
