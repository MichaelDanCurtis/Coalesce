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
                  <th className="px-4 py-2">Tier</th>
                  <th className="px-4 py-2 text-right">Input $/M</th>
                  <th className="px-4 py-2 text-right">Output $/M</th>
                  <th className="px-4 py-2 text-right">Marginal</th>
                  <th className="px-4 py-2">Features</th>
                  <th className="px-4 py-2">Status</th>
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
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      ))}
    </div>
  );
}
