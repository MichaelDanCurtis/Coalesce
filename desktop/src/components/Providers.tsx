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

export default function Providers() {
  const fetchModels = useCallback(() => api.getModels(), []);
  const fetchProviders = useCallback(() => api.getProviders(), []);
  const { data: modelsData, loading } = useApi(fetchModels);
  const { data: providersData } = useApi(fetchProviders);
  const [filter, setFilter] = useState("");

  const models = (modelsData as any)?.data ?? [];
  const providers = (providersData as any)?.providers ?? [];

  // Filter models by search term (substring match on id, provider, tier)
  const filteredModels = useMemo(() => {
    if (!filter.trim()) return models;
    const q = filter.toLowerCase();
    return models.filter((m: any) =>
      m.id?.toLowerCase().includes(q) ||
      m.owned_by?.toLowerCase().includes(q) ||
      m.quality_tier?.toLowerCase().includes(q)
    );
  }, [models, filter]);

  // Group filtered models by provider
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
            {providers.map((p: any) => (
              <div key={p.name} className="card">
                <div className="flex items-center justify-between mb-2">
                  <p className="text-sm font-medium">{displayName(p.name)}</p>
                  <span className={`inline-block w-2 h-2 rounded-full ${
                    p.is_available ? "bg-emerald-500" : "bg-red-500"
                  }`} title={p.is_available ? "Online" : "Offline"} />
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
                  {p.circuit_breaker && (
                    <div className="flex justify-between">
                      <span>Health</span>
                      <span className={
                        p.circuit_breaker.state === "Healthy" ? "text-emerald-400" :
                        p.circuit_breaker.state === "Recovering" ? "text-amber-400" :
                        "text-red-400"
                      }>{p.circuit_breaker.state}</span>
                    </div>
                  )}
                </div>
              </div>
            ))}
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
                {providerModels.map((m: any) => (
                  <tr key={m.id} className="border-b border-themed-faint">
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
                      <span
                        className={
                          m.circuit_breaker === "Closed" || m.circuit_breaker === "Healthy"
                            ? "badge-green"
                            : m.circuit_breaker === "HalfOpen" || m.circuit_breaker === "Recovering"
                            ? "badge-yellow"
                            : "badge-red"
                        }
                      >
                        {m.circuit_breaker === "Closed" ? "Healthy" :
                         m.circuit_breaker === "Open" ? "Unavailable" :
                         m.circuit_breaker === "HalfOpen" ? "Recovering" :
                         m.circuit_breaker}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ))}
    </div>
  );
}
