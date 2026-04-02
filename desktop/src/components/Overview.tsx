import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api/client";
import { useApi } from "../hooks/useApi";
import type { ConfigProfile } from "../types";

export default function Overview() {
  const fetchStats = useCallback(() => api.getStats(), []);
  const fetchHealth = useCallback(() => api.getHealth(), []);
  const { data: statsData, loading: statsLoading, error: statsError } = useApi(fetchStats);
  const { data: healthData } = useApi(fetchHealth);

  if (statsLoading) {
    return <div className="flex items-center justify-center h-64 text-secondary">Loading...</div>;
  }
  if (statsError) {
    return (
      <div className="card border-red-900 text-center py-12">
        <p className="text-red-400 text-lg font-medium">Proxy Offline</p>
        <p className="text-secondary mt-2 text-sm">
          Start the proxy with: <code className="bg-tertiary px-2 py-1 rounded">coalesce serve</code>
        </p>
      </div>
    );
  }

  const stats = (statsData as any)?.stats;
  const recent = (statsData as any)?.recent_requests ?? [];
  const health = healthData as any;

  const freeRequests = stats?.free_requests ?? 0;
  const moneySaved = freeRequests * 0.003;

  const latencies = recent.slice(0, 10).map((r: any) => r.latency_ms ?? 0).reverse();

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-semibold">Overview</h2>
      </div>

      {/* Profile Switcher */}
      <ProfileSwitcher />

      {/* Stats Row */}
      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <StatCard label="Total Requests" value={stats?.total_requests ?? 0} />
        <StatCard
          label="Success Rate"
          value={`${((stats?.success_rate ?? 1) * 100).toFixed(1)}%`}
        />
        <MoneySavedCard target={moneySaved} />
        <LatencySparkCard
          label="Avg Latency"
          value={`${(stats?.avg_latency_ms ?? 0).toFixed(0)}ms`}
          latencies={latencies}
        />
      </div>

      {/* Provider Health */}
      {health?.circuit_breakers && (
        <div>
          <h3 className="text-sm font-medium text-secondary mb-3">Providers</h3>
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-3">
            {(health.circuit_breakers as any[]).map((cb: any) => (
              <div key={cb.provider} className="card flex items-center gap-3">
                <div
                  className={`h-3 w-3 rounded-full ${
                    cb.state === "Closed"
                      ? "bg-emerald-500"
                      : cb.state === "HalfOpen"
                      ? "bg-amber-500"
                      : "bg-red-500"
                  }`}
                />
                <div>
                  <p className="text-sm font-medium">{cb.provider}</p>
                  <p className="text-xs text-secondary">
                    {cb.total_requests} req · {cb.total_failures} fail
                  </p>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Recent Requests */}
      {recent.length > 0 && (
        <div>
          <h3 className="text-sm font-medium text-secondary mb-3">Recent Requests</h3>
          <div className="card overflow-hidden p-0">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-themed text-left text-xs text-secondary">
                  <th className="px-4 py-2">Tier</th>
                  <th className="px-4 py-2">Provider</th>
                  <th className="px-4 py-2">Model</th>
                  <th className="px-4 py-2 text-right">Cost</th>
                  <th className="px-4 py-2 text-right">Latency</th>
                  <th className="px-4 py-2">Status</th>
                </tr>
              </thead>
              <tbody>
                {recent.slice(0, 10).map((req: any, i: number) => (
                  <tr key={i} className="border-b border-themed-faint">
                    <td className="px-4 py-2">
                      <span className="badge-green">{req.tier}</span>
                    </td>
                    <td className="px-4 py-2">{req.provider}</td>
                    <td className="px-4 py-2 font-mono text-xs text-secondary">{req.model}</td>
                    <td className="px-4 py-2 text-right">
                      <span className={req.cost_usd > 0 ? "text-amber-400" : "text-emerald-400"}>
                        ${(req.cost_usd ?? 0).toFixed(6)}
                      </span>
                    </td>
                    <td className="px-4 py-2 text-right text-secondary">
                      {req.latency_ms ?? "-"}ms
                    </td>
                    <td className="px-4 py-2">
                      {req.success ? (
                        <span className="badge-green">OK</span>
                      ) : (
                        <span className="badge-red">FAIL</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}

// ==================== Profile Switcher ====================

function ProfileSwitcher() {
  const [profiles, setProfiles] = useState<ConfigProfile[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [showSave, setShowSave] = useState(false);
  const [showManage, setShowManage] = useState(false);
  const [newName, setNewName] = useState("");
  const [newDesc, setNewDesc] = useState("");
  const [message, setMessage] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const data = await api.getConfigProfiles();
      setProfiles(data.profiles);
      setActive(data.active || null);
    } catch {}
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  const handleSave = async () => {
    if (!newName.trim()) return;
    setLoading(true);
    try {
      const res = await api.saveProfile(newName.trim(), newDesc.trim() || undefined);
      if (res.status === "ok") {
        setMessage(`Profile "${newName}" saved`);
        setShowSave(false);
        setNewName("");
        setNewDesc("");
        refresh();
      } else {
        setMessage(res.error || "Failed to save");
      }
    } catch (e: any) {
      setMessage(e.message);
    } finally {
      setLoading(false);
    }
  };

  const handleActivate = async (name: string) => {
    setLoading(true);
    try {
      const res = await api.activateProfile(name);
      if (res.status === "ok") {
        setMessage(`Profile "${name}" activated (${res.providers_applied} providers). Restart proxy to fully reload.`);
        setActive(name);
        refresh();
      } else {
        setMessage(res.error || "Failed to activate");
      }
    } catch (e: any) {
      setMessage(e.message);
    } finally {
      setLoading(false);
    }
  };

  const handleDelete = async (name: string) => {
    setLoading(true);
    try {
      await api.deleteProfile(name);
      setMessage(`Deleted "${name}"`);
      refresh();
    } catch (e: any) {
      setMessage(e.message);
    } finally {
      setLoading(false);
    }
  };

  const handleExport = async (name: string) => {
    try {
      const res = await api.exportProfile(name);
      if (res.profile) {
        const blob = new Blob([JSON.stringify(res.profile, null, 2)], { type: "application/json" });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = `coalesce-profile-${name}.json`;
        a.click();
        URL.revokeObjectURL(url);
      }
    } catch (e: any) {
      setMessage(e.message);
    }
  };

  const handleImport = async () => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".json";
    input.onchange = async (e) => {
      const file = (e.target as HTMLInputElement).files?.[0];
      if (!file) return;
      try {
        const text = await file.text();
        const profile = JSON.parse(text);
        const res = await api.importProfile(profile);
        if (res.status === "ok") {
          setMessage(`Imported profile "${res.name}"`);
          refresh();
        } else {
          setMessage(res.error || "Import failed");
        }
      } catch (e: any) {
        setMessage(e.message);
      }
    };
    input.click();
  };

  // Auto-dismiss message
  useEffect(() => {
    if (message) {
      const t = setTimeout(() => setMessage(null), 5000);
      return () => clearTimeout(t);
    }
  }, [message]);

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <div className="flex items-center gap-3">
          <h3 className="text-sm font-medium">Configuration Profile</h3>
          {active && (
            <span className="badge-green text-xs">{active}</span>
          )}
          {!active && profiles.length > 0 && (
            <span className="text-xs text-secondary">No profile active</span>
          )}
        </div>
        <div className="flex gap-2">
          <button
            className="btn-ghost text-xs"
            onClick={() => setShowSave(!showSave)}
          >
            Save Current
          </button>
          <button
            className="btn-ghost text-xs"
            onClick={handleImport}
          >
            Import
          </button>
          {profiles.length > 0 && (
            <button
              className="btn-ghost text-xs"
              onClick={() => setShowManage(!showManage)}
            >
              {showManage ? "Hide" : "Manage"} ({profiles.length})
            </button>
          )}
        </div>
      </div>

      {/* Message */}
      {message && (
        <div className="text-xs text-brand-400 bg-brand-500/10 rounded px-3 py-2 mb-3">
          {message}
        </div>
      )}

      {/* Save New Profile Form */}
      {showSave && (
        <div className="border border-themed rounded p-3 mb-3 space-y-2">
          <p className="text-xs text-secondary">Save current provider configuration as a reusable profile</p>
          <input
            type="text"
            placeholder="Profile name (e.g., 'Copilot Primary', 'Google Only')"
            className="input-field w-full"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleSave()}
          />
          <input
            type="text"
            placeholder="Description (optional)"
            className="input-field w-full"
            value={newDesc}
            onChange={(e) => setNewDesc(e.target.value)}
          />
          <div className="flex gap-2">
            <button
              className="btn-primary text-xs"
              disabled={loading || !newName.trim()}
              onClick={handleSave}
            >
              {loading ? "Saving..." : "Save Profile"}
            </button>
            <button
              className="btn-ghost text-xs"
              onClick={() => setShowSave(false)}
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {/* Quick Switch - always show if profiles exist */}
      {profiles.length > 0 && !showManage && (
        <div className="flex flex-wrap gap-2">
          {profiles.map((p) => (
            <button
              key={p.name}
              className={`px-3 py-1.5 rounded text-xs font-medium transition-colors ${
                active === p.name
                  ? "bg-brand-500/15 text-brand-400 ring-1 ring-brand-500/20"
                  : "bg-tertiary hover:bg-hover"
              }`}
              disabled={loading}
              onClick={() => active !== p.name && handleActivate(p.name)}
              title={p.description || `${p.provider_count} providers`}
            >
              {p.name}
              <span className="ml-1 text-secondary">({p.provider_count})</span>
            </button>
          ))}
        </div>
      )}

      {/* Manage Panel */}
      {showManage && (
        <div className="space-y-2">
          {profiles.map((p) => (
            <div
              key={p.name}
              className={`flex items-center justify-between p-3 rounded border ${
                active === p.name
                  ? "border-brand-500/30 bg-brand-500/5"
                  : "border-themed-faint bg-surface-alt"
              }`}
            >
              <div>
                <div className="flex items-center gap-2">
                  <span className="text-sm font-medium">{p.name}</span>
                  {active === p.name && (
                    <span className="badge-green text-xs">Active</span>
                  )}
                </div>
                <p className="text-xs text-secondary">
                  {p.description || `${p.provider_count} providers`}
                  {" · "}
                  {new Date(p.updated_at * 1000).toLocaleDateString()}
                </p>
              </div>
              <div className="flex gap-1">
                {active !== p.name && (
                  <button
                    className="btn-primary text-xs"
                    disabled={loading}
                    onClick={() => handleActivate(p.name)}
                  >
                    Activate
                  </button>
                )}
                <button
                  className="btn-ghost text-xs"
                  onClick={() => handleExport(p.name)}
                >
                  Export
                </button>
                <button
                  className="btn-ghost text-xs text-red-400 hover:text-red-300"
                  disabled={loading}
                  onClick={() => handleDelete(p.name)}
                >
                  Delete
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {profiles.length === 0 && !showSave && (
        <p className="text-xs text-secondary">
          No profiles saved yet. Save your current configuration to quickly switch between different setups.
        </p>
      )}
    </div>
  );
}

// ==================== Stat Cards ====================

function MoneySavedCard({ target }: { target: number }) {
  const [display, setDisplay] = useState(0);
  const rafRef = useRef<number>(0);

  useEffect(() => {
    if (target <= 0) {
      setDisplay(0);
      return;
    }
    const start = performance.now();
    const duration = 1000;
    const animate = (now: number) => {
      const elapsed = now - start;
      const progress = Math.min(elapsed / duration, 1);
      const eased = 1 - Math.pow(1 - progress, 3);
      setDisplay(eased * target);
      if (progress < 1) {
        rafRef.current = requestAnimationFrame(animate);
      }
    };
    rafRef.current = requestAnimationFrame(animate);
    return () => cancelAnimationFrame(rafRef.current);
  }, [target]);

  return (
    <div className="card">
      <p className="text-xs text-secondary">Money Saved</p>
      <p className="mt-1 text-2xl font-semibold text-emerald-400">
        ${display.toFixed(4)}
      </p>
    </div>
  );
}

function LatencySparkCard({
  label,
  value,
  latencies,
}: {
  label: string;
  value: string;
  latencies: number[];
}) {
  const sparkline = buildSparklinePath(latencies, 60, 20);

  return (
    <div className="card">
      <p className="text-xs text-secondary">{label}</p>
      <div className="flex items-center gap-2 mt-1">
        <p className="text-2xl font-semibold">{value}</p>
        {latencies.length >= 2 && (
          <svg width={60} height={20} className="flex-shrink-0">
            <polyline
              points={sparkline}
              fill="none"
              stroke="var(--brand-400)"
              strokeWidth="1.5"
              strokeLinejoin="round"
            />
          </svg>
        )}
      </div>
    </div>
  );
}

function buildSparklinePath(data: number[], w: number, h: number): string {
  if (data.length < 2) return "";
  const max = Math.max(...data, 1);
  const min = Math.min(...data, 0);
  const range = max - min || 1;
  const stepX = w / (data.length - 1);
  return data
    .map((v, i) => {
      const x = (i * stepX).toFixed(1);
      const y = (h - ((v - min) / range) * (h - 2) - 1).toFixed(1);
      return `${x},${y}`;
    })
    .join(" ");
}

function StatCard({
  label,
  value,
  color,
}: {
  label: string;
  value: string | number;
  color?: string;
}) {
  return (
    <div className="card">
      <p className="text-xs text-secondary">{label}</p>
      <p className={`mt-1 text-2xl font-semibold ${color ?? ""}`}>{value}</p>
    </div>
  );
}
