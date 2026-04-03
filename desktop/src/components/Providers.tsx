import { useCallback, useState, useMemo } from "react";
import { api } from "../api/client";
import { useApi } from "../hooks/useApi";
import { displayName } from "./ProviderConfig";

function billingLabel(billing: string): string {
  if (!billing) return "Unknown";
  if (billing === "per_token" || billing === "per-token" || billing === "PerToken") return "Per Token";
  if (billing === "local" || billing === "Local") return "Local (Free)";
  if (billing === "free" || billing === "FreeIncluded") return "Free";
  if (billing === "unlimited" || billing === "UnlimitedSubscription") return "Unlimited";
  if (billing.includes("QuotaOnly")) return "Quota Only";
  if (billing.includes("QuotaMetered")) return "Quota + Token";
  if (billing.includes("QuotaRefreshing")) return "Quota (Refreshing)";
  if (billing.includes("QuotaMonthly")) return "Monthly Quota";
  if (billing.includes("FreeTierCredits")) return "Free Credits";
  return billing;
}

function formatTokens(n: number | undefined): string {
  if (!n) return "0";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return n.toString();
}

function cbLabel(raw: string): string {
  if (raw === "Closed") return "Healthy";
  if (raw === "Open") return "Unavailable";
  if (raw === "HalfOpen") return "Recovering";
  return raw;
}

export default function Providers() {
  const fetchModels = useCallback(() => api.getModels(), []);
  const fetchProviders = useCallback(() => api.getProviders(), []);
  const { data: modelsData, loading, refresh: refreshModels } = useApi(fetchModels);
  const { data: providersData, refresh: refreshProviders } = useApi(fetchProviders);
  const [filter, setFilter] = useState("");
  const [refreshing, setRefreshing] = useState<Record<string, boolean>>({});
  const [editingModel, setEditingModel] = useState<any>(null);
  const [editOverrides, setEditOverrides] = useState<Record<string, string>>({});
  const [editSaving, setEditSaving] = useState(false);

  const openModelEditor = async (m: any) => {
    setEditingModel(m);
    try {
      const resp = await api.getModelOverrides(m.owned_by, m.id);
      setEditOverrides(resp.overrides || {});
    } catch { setEditOverrides({}); }
  };

  const saveOverrides = async () => {
    if (!editingModel) return;
    setEditSaving(true);
    try {
      // Only send fields that differ from current model values
      const toSave: Record<string, string> = { ...editOverrides };
      await api.setModelOverrides(editingModel.owned_by, editingModel.id, toSave);
      await refreshModels();
      setEditingModel(null);
    } catch (e) { console.error("Save overrides failed:", e); }
    setEditSaving(false);
  };

  const clearOverrides = async () => {
    if (!editingModel) return;
    setEditSaving(true);
    try {
      await api.clearModelOverrides(editingModel.owned_by, editingModel.id);
      await refreshModels();
      setEditingModel(null);
    } catch (e) { console.error("Clear overrides failed:", e); }
    setEditSaving(false);
  };

  const refreshProvider = async (name: string) => {
    setRefreshing(prev => ({ ...prev, [name]: true }));
    try {
      await api.refreshProvider(name);
      await Promise.all([refreshModels(), refreshProviders()]);
    } catch (e) {
      console.error("Refresh provider failed:", e);
    }
    setRefreshing(prev => ({ ...prev, [name]: false }));
  };

  // Optimistic overrides: track what the user clicked before server confirms
  const [providerOverrides, setProviderOverrides] = useState<Record<string, boolean>>({});
  const [modelOverrides, setModelOverrides] = useState<Record<string, boolean>>({});

  const models = (modelsData as any)?.data ?? [];
  const providers = (providersData as any)?.providers ?? [];

  const toggleProvider = async (name: string, currentlyDisabled: boolean) => {
    const newDisabled = !currentlyDisabled;
    // Optimistic: immediately show the new state
    setProviderOverrides(prev => ({ ...prev, [name]: newDisabled }));
    try {
      await api.toggleProvider(name, currentlyDisabled);
      // Refresh from server, then clear override
      await Promise.all([refreshProviders(), refreshModels()]);
    } catch (e) {
      console.error("Toggle provider failed:", e);
    }
    setProviderOverrides(prev => { const n = { ...prev }; delete n[name]; return n; });
  };

  const toggleModel = async (provider: string, modelId: string, currentlyDisabled: boolean) => {
    const key = `${provider}::${modelId}`;
    const newDisabled = !currentlyDisabled;
    setModelOverrides(prev => ({ ...prev, [key]: newDisabled }));
    try {
      await api.toggleModel(provider, modelId, currentlyDisabled);
      await Promise.all([refreshModels(), refreshProviders()]);
    } catch (e) {
      console.error("Toggle model failed:", e);
    }
    setModelOverrides(prev => { const n = { ...prev }; delete n[key]; return n; });
  };

  // Resolve effective disabled state with optimistic overrides
  const isProviderDisabled = (name: string, serverDisabled: boolean) =>
    name in providerOverrides ? providerOverrides[name] : serverDisabled;

  const isModelDisabled = (provider: string, modelId: string, serverDisabled: boolean) => {
    const key = `${provider}::${modelId}`;
    return key in modelOverrides ? modelOverrides[key] : serverDisabled;
  };

  const filteredModels = useMemo(() => {
    if (!filter.trim()) return models;
    const q = filter.toLowerCase();
    return models.filter((m: any) =>
      m.id?.toLowerCase().includes(q) ||
      m.owned_by?.toLowerCase().includes(q) ||
      m.quality_tier?.toLowerCase().includes(q)
    );
  }, [models, filter]);

  const byProvider: Record<string, any[]> = {};
  for (const m of filteredModels) {
    const p = m.owned_by ?? "unknown";
    (byProvider[p] ??= []).push(m);
  }

  if (loading) {
    return <div className="flex items-center justify-center h-64 text-secondary">Loading...</div>;
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between gap-4">
        <h2 className="text-xl font-semibold">Providers & Models</h2>
        <input
          type="text"
          placeholder="Filter models... (e.g. opus, gemini, copilot)"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className="w-80 rounded-lg border border-themed bg-tertiary px-3 py-1.5 text-sm focus:border-brand-500 focus:outline-none"
        />
      </div>

      {/* Provider Status Cards */}
      {providers.length > 0 && (
        <div>
          <h3 className="text-sm font-medium text-secondary mb-3">Provider Status</h3>
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-3 xl:grid-cols-4">
            {providers.map((p: any) => {
              const disabled = isProviderDisabled(p.name, !!p.is_disabled);
              const cbState = p.circuit_breaker?.state ?? "Unknown";
              return (
                <div key={p.name} className={`card transition-opacity duration-200 ${disabled ? "opacity-50" : ""}`}>
                  <div className="flex items-center justify-between mb-2">
                    <div className="flex items-center gap-1.5">
                      <p className="text-sm font-medium">{displayName(p.name)}</p>
                      <button
                        onClick={() => refreshProvider(p.name)}
                        disabled={!!refreshing[p.name]}
                        className={`cursor-pointer text-secondary hover:text-primary transition-colors p-0.5 ${refreshing[p.name] ? "animate-spin" : ""}`}
                        title="Refresh models from API"
                      >
                        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                        </svg>
                      </button>
                    </div>
                    <button
                      onClick={() => toggleProvider(p.name, disabled)}
                      className={`cursor-pointer text-[10px] px-1.5 py-0.5 rounded-full font-medium transition-all hover:ring-1 ${
                        disabled
                          ? "badge-red hover:ring-red-400/40"
                          : cbState === "Healthy"
                          ? "badge-green hover:ring-emerald-400/40"
                          : cbState === "Recovering"
                          ? "badge-yellow hover:ring-amber-400/40"
                          : "badge-red hover:ring-red-400/40"
                      }`}
                      title={disabled ? "Click to re-enable provider" : "Click to bypass provider"}
                    >
                      {disabled ? "Bypassed" : cbState}
                    </button>
                  </div>
                  <div className="space-y-1 text-xs text-secondary">
                    <div className="flex justify-between">
                      <span>Billing</span>
                      <span className="text-primary font-medium">{billingLabel(p.billing)}</span>
                    </div>
                    <div className="flex justify-between">
                      <span>Models</span>
                      <span>{p.model_count}</span>
                    </div>
                    <div className="flex justify-between">
                      <span>Requests</span>
                      <span>{p.total_requests ?? 0}</span>
                    </div>
                    <div className="flex justify-between">
                      <span>Tokens</span>
                      <span>
                        {formatTokens(p.total_input_tokens)} in / {formatTokens(p.total_output_tokens)} out
                      </span>
                    </div>
                    {(p.total_cost_usd ?? 0) > 0 && (
                      <div className="flex justify-between">
                        <span>Cost</span>
                        <span className="text-amber-400">${p.total_cost_usd.toFixed(4)}</span>
                      </div>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Models by Provider */}
      {filter && filteredModels.length === 0 && (
        <p className="text-sm text-secondary">No models match "{filter}"</p>
      )}

      {Object.entries(byProvider).map(([provider, providerModels]) => (
        <div key={provider}>
          <h3 className="text-sm font-medium text-secondary mb-3">
            {provider}{" "}
            <span className="opacity-50">({providerModels.length} models)</span>
          </h3>
          <div className="card overflow-hidden p-0">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-themed text-left text-xs text-secondary">
                  <th className="px-4 py-2">Model</th>
                  <th className="px-4 py-2">Family</th>
                  <th className="px-4 py-2">Tier</th>
                  <th className="px-4 py-2 text-right">Input $/M</th>
                  <th className="px-4 py-2 text-right">Output $/M</th>
                  <th className="px-4 py-2 text-right">Marginal</th>
                  <th className="px-4 py-2">Features</th>
                  <th className="px-4 py-2">Status</th>
                  <th className="px-4 py-2"></th>
                </tr>
              </thead>
              <tbody>
                {providerModels.map((m: any) => {
                  const pDisabled = isProviderDisabled(provider, providers.find((p: any) => p.name === provider)?.is_disabled ?? false);
                  const disabled = isModelDisabled(provider, m.id, !!m.is_disabled);
                  const cbRaw = m.circuit_breaker ?? "Unknown";
                  const cbText = cbLabel(cbRaw);

                  return (
                    <tr key={m.id} className={`border-b border-themed-faint transition-opacity duration-200 ${disabled ? "opacity-40" : ""}`}>
                      <td className="px-4 py-2 font-mono text-xs">{m.id}</td>
                      <td className="px-4 py-2 font-mono text-xs text-secondary" title={m.canonical_family || "—"}>
                        {m.canonical_family ? m.canonical_family.length > 20 ? m.canonical_family.slice(0, 18) + "…" : m.canonical_family : "—"}
                      </td>
                      <td className="px-4 py-2">
                        <span className="badge-green">{m.quality_tier}</span>
                      </td>
                      <td className="px-4 py-2 text-right text-secondary">
                        ${m.pricing?.input_per_m?.toFixed(2) ?? "0.00"}
                      </td>
                      <td className="px-4 py-2 text-right text-secondary">
                        ${m.pricing?.output_per_m?.toFixed(2) ?? "0.00"}
                      </td>
                      <td className="px-4 py-2 text-right">
                        <span
                          className={
                            m.marginal_cost?.is_free
                              ? "text-emerald-400"
                              : m.marginal_cost?.is_available
                              ? "text-amber-400"
                              : "text-red-400"
                          }
                        >
                          {m.marginal_cost?.is_free
                            ? "$0"
                            : `$${m.marginal_cost?.usd?.toFixed(6) ?? "?"}`}
                        </span>
                      </td>
                      <td className="px-4 py-2 space-x-1">
                        {m.reasoning && <span className="badge-yellow">reason</span>}
                        {m.vision && <span className="badge-green">vision</span>}
                        {m.tool_calling && <span className="badge-green">tools</span>}
                      </td>
                      <td className="px-4 py-2">
                        {pDisabled && !m.is_disabled ? (
                          <span className="badge-red opacity-60" title="Provider is bypassed">Bypassed</span>
                        ) : (
                          <button
                            onClick={() => toggleModel(provider, m.id, disabled)}
                            className={`cursor-pointer transition-all hover:ring-1 ${
                              disabled
                                ? "badge-red hover:ring-red-400/40"
                                : cbText === "Healthy"
                                ? "badge-green hover:ring-emerald-400/40"
                                : cbText === "Recovering"
                                ? "badge-yellow hover:ring-amber-400/40"
                                : "badge-red hover:ring-red-400/40"
                            }`}
                            title={disabled ? "Click to re-enable model" : "Click to bypass model from routing"}
                          >
                            {disabled ? "Bypassed" : cbText}
                          </button>
                        )}
                      </td>
                      <td className="px-2 py-2">
                        <button
                          onClick={() => openModelEditor(m)}
                          className="cursor-pointer text-secondary hover:text-primary transition-colors"
                          title="Edit model overrides"
                        >
                          <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                          </svg>
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      ))}
      {/* Model Override Editor Modal */}
      {editingModel && (
        <div className="fixed inset-0 bg-black/90 z-50 flex items-center justify-center" onClick={() => setEditingModel(null)}>
          <div className="card rounded-xl shadow-2xl w-[480px] max-h-[90vh] overflow-y-auto" onClick={e => e.stopPropagation()}>
            <div className="p-5 border-b border-themed">
              <h3 className="text-lg font-semibold">Edit Model</h3>
              <p className="text-xs text-secondary font-mono mt-1">{editingModel.owned_by} / {editingModel.id}</p>
              {editingModel.canonical_family && (
                <p className="text-xs text-secondary mt-0.5">Family: <span className="text-primary">{editingModel.canonical_family}</span></p>
              )}
            </div>
            <div className="p-5 space-y-4">
              {/* Quality Tier */}
              <div>
                <label className="block text-xs text-secondary mb-1">Quality Tier</label>
                <select
                  value={editOverrides.quality_tier ?? ""}
                  onChange={e => setEditOverrides(p => e.target.value ? { ...p, quality_tier: e.target.value } : (() => { const n = { ...p }; delete n.quality_tier; return n; })())}
                  className="w-full rounded-lg border border-themed bg-tertiary px-3 py-1.5 text-sm"
                >
                  <option value="">Default ({editingModel.quality_tier})</option>
                  <option value="simple">Simple</option>
                  <option value="medium">Medium</option>
                  <option value="complex">Complex</option>
                  <option value="reasoning">Reasoning</option>
                </select>
              </div>

              {/* Boolean toggles */}
              {["reasoning", "vision", "tool_calling"].map(field => {
                const current = editingModel[field] ? "true" : "false";
                const label = field === "tool_calling" ? "Tool Calling" : field.charAt(0).toUpperCase() + field.slice(1);
                return (
                  <div key={field} className="flex items-center justify-between">
                    <label className="text-sm">{label}</label>
                    <div className="flex items-center gap-2">
                      <span className="text-xs text-secondary">({current})</span>
                      <select
                        value={editOverrides[field] ?? ""}
                        onChange={e => setEditOverrides(p => e.target.value ? { ...p, [field]: e.target.value } : (() => { const n = { ...p }; delete n[field]; return n; })())}
                        className="rounded-lg border border-themed bg-tertiary px-2 py-1 text-sm"
                      >
                        <option value="">Default</option>
                        <option value="true">Yes</option>
                        <option value="false">No</option>
                      </select>
                    </div>
                  </div>
                );
              })}

              {/* Canonical Family */}
              <div>
                <label className="block text-xs text-secondary mb-1">Canonical Family</label>
                <input
                  type="text"
                  value={editOverrides.canonical_family ?? ""}
                  placeholder={editingModel.canonical_family || "auto-derived"}
                  onChange={e => setEditOverrides(p => e.target.value ? { ...p, canonical_family: e.target.value } : (() => { const n = { ...p }; delete n.canonical_family; return n; })())}
                  className="w-full rounded-lg border border-themed bg-tertiary px-3 py-1.5 text-sm font-mono"
                />
              </div>

              {/* Pricing */}
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs text-secondary mb-1">Input $/M</label>
                  <input
                    type="number" step="0.01"
                    value={editOverrides.input_price_per_m ?? ""}
                    placeholder={editingModel.pricing?.input_per_m?.toFixed(2)}
                    onChange={e => setEditOverrides(p => e.target.value ? { ...p, input_price_per_m: e.target.value } : (() => { const n = { ...p }; delete n.input_price_per_m; return n; })())}
                    className="w-full rounded-lg border border-themed bg-tertiary px-3 py-1.5 text-sm font-mono"
                  />
                </div>
                <div>
                  <label className="block text-xs text-secondary mb-1">Output $/M</label>
                  <input
                    type="number" step="0.01"
                    value={editOverrides.output_price_per_m ?? ""}
                    placeholder={editingModel.pricing?.output_per_m?.toFixed(2)}
                    onChange={e => setEditOverrides(p => e.target.value ? { ...p, output_price_per_m: e.target.value } : (() => { const n = { ...p }; delete n.output_price_per_m; return n; })())}
                    className="w-full rounded-lg border border-themed bg-tertiary px-3 py-1.5 text-sm font-mono"
                  />
                </div>
              </div>

              {/* Capabilities display (read-only) */}
              {editingModel.capabilities && (
                <div className="text-xs text-secondary border-t border-themed pt-3 mt-3">
                  <p className="font-medium text-primary mb-1">API Capabilities (read-only)</p>
                  {editingModel.capabilities.vendor && <p>Vendor: {editingModel.capabilities.vendor}</p>}
                  {editingModel.capabilities.model_picker_category && <p>Category: {editingModel.capabilities.model_picker_category}</p>}
                  {editingModel.capabilities.reasoning_effort_levels && (
                    <p>Reasoning efforts: {editingModel.capabilities.reasoning_effort_levels.join(", ")}</p>
                  )}
                  {editingModel.capabilities.thinking_budget && (
                    <p>Thinking budget: {editingModel.capabilities.thinking_budget[0]}–{editingModel.capabilities.thinking_budget[1]} tokens</p>
                  )}
                  {editingModel.capabilities.supported_endpoints && (
                    <p>Endpoints: {editingModel.capabilities.supported_endpoints.join(", ")}</p>
                  )}
                </div>
              )}
            </div>
            <div className="p-4 border-t border-themed flex items-center justify-between">
              <button
                onClick={clearOverrides}
                disabled={editSaving || Object.keys(editOverrides).length === 0}
                className="text-xs text-red-400 hover:text-red-300 disabled:opacity-30 cursor-pointer"
              >
                Clear All Overrides
              </button>
              <div className="flex gap-2">
                <button onClick={() => setEditingModel(null)} className="px-3 py-1.5 text-sm rounded-lg border border-themed hover:bg-tertiary cursor-pointer">
                  Cancel
                </button>
                <button
                  onClick={saveOverrides}
                  disabled={editSaving || Object.keys(editOverrides).length === 0}
                  className="px-3 py-1.5 text-sm rounded-lg bg-brand-600 hover:bg-brand-500 text-white disabled:opacity-50 cursor-pointer"
                >
                  {editSaving ? "Saving..." : "Save Overrides"}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
