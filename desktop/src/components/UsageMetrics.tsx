import { useCallback } from "react";
import { api } from "../api/client";
import { useApi } from "../hooks/useApi";

export default function UsageMetrics() {
  const fetchProviders = useCallback(() => api.getProviders(), []);
  const fetchQuotas = useCallback(() => api.getQuotas(), []);
  const fetchCosts = useCallback(() => api.getCosts(30), []);
  const { data: providersData, loading } = useApi(fetchProviders);
  const { data: quotasData } = useApi(fetchQuotas);
  const { data: costsData } = useApi(fetchCosts);

  if (loading) {
    return <div className="flex items-center justify-center h-64 text-secondary">Loading...</div>;
  }

  const providers = (providersData as any)?.providers ?? [];
  const quotas = (quotasData as any)?.quotas ?? [];
  const costs = costsData as any;
  const byProvider = costs?.by_provider ?? [];

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Provider Usage</h2>

      {providers.length === 0 ? (
        <div className="card text-center py-12 text-secondary">
          <p>No providers configured. Start the proxy to see usage metrics.</p>
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          {providers.map((p: any) => {
            const quota = quotas.find((q: any) => q.provider === p.name);
            const costInfo = byProvider.find((c: any) => c.provider === p.name);

            return (
              <ProviderCard
                key={p.name}
                provider={p}
                quota={quota}
                costInfo={costInfo}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

function ProviderCard({
  provider,
  quota,
  costInfo,
}: {
  provider: any;
  quota: any;
  costInfo: any;
}) {
  const isHealthy = provider.is_available;
  const cbState = provider.circuit_breaker?.state ?? "Unknown";

  return (
    <div className="card space-y-3">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <div className={`h-3 w-3 rounded-full ${
            isHealthy ? "bg-emerald-500" : "bg-red-500"
          }`} />
          <h3 className="font-medium">{provider.name}</h3>
        </div>
        <span className={
          cbState === "Closed" ? "badge-green" :
          cbState === "HalfOpen" ? "badge-yellow" : "badge-red"
        }>
          {cbState}
        </span>
      </div>

      {/* Stats Grid */}
      <div className="grid grid-cols-3 gap-3 text-center">
        <div>
          <p className="text-xs text-secondary">Models</p>
          <p className="text-lg font-semibold">{provider.model_count}</p>
        </div>
        <div>
          <p className="text-xs text-secondary">Requests</p>
          <p className="text-lg font-semibold">{costInfo?.requests ?? 0}</p>
        </div>
        <div>
          <p className="text-xs text-secondary">Cost</p>
          <p className={`text-lg font-semibold ${
            (costInfo?.total_cost_usd ?? 0) > 0 ? "text-amber-400" : "text-emerald-400"
          }`}>
            ${(costInfo?.total_cost_usd ?? 0).toFixed(4)}
          </p>
        </div>
      </div>

      {/* Billing Type */}
      <div className="flex items-center justify-between text-xs">
        <span className="text-secondary">Billing</span>
        <span className="badge-green">{provider.billing}</span>
      </div>

      {/* Quota Burndown Bar */}
      {quota && <QuotaBurndown quota={quota} costInfo={costInfo} />}

      {/* Token Usage */}
      {costInfo && (costInfo.input_tokens > 0 || costInfo.output_tokens > 0) && (
        <div className="flex items-center justify-between text-xs text-secondary">
          <span>Tokens: {formatNumber(costInfo.input_tokens)} in / {formatNumber(costInfo.output_tokens)} out</span>
          <span>Avg {costInfo.avg_latency_ms.toFixed(0)}ms</span>
        </div>
      )}
    </div>
  );
}

function QuotaBurndown({ quota, costInfo }: { quota: any; costInfo: any }) {
  const used = quota.used ?? 0;
  const total = used + (quota.remaining ?? 0);
  const usagePct = total > 0 ? (used / total) * 100 : 0;

  // Color based on usage percentage
  const barColor =
    usagePct > 80
      ? "bg-red-500"
      : usagePct > 50
      ? "bg-amber-500"
      : "bg-emerald-500";

  const textColor =
    usagePct > 80
      ? "text-red-400"
      : usagePct > 50
      ? "text-amber-400"
      : "text-emerald-400";

  // Projected depletion based on usage rate
  let depletionText = "";
  if (quota.remaining != null && quota.remaining > 0 && costInfo?.requests > 0) {
    // Estimate: assume requests happened over ~24h window
    const ratePerHour = costInfo.requests / 24;
    if (ratePerHour > 0) {
      const hoursRemaining = quota.remaining / ratePerHour;
      if (hoursRemaining < 1) {
        depletionText = `~${Math.round(hoursRemaining * 60)}m remaining`;
      } else if (hoursRemaining < 48) {
        depletionText = `~${Math.round(hoursRemaining)}h remaining`;
      } else {
        depletionText = `~${Math.round(hoursRemaining / 24)}d remaining`;
      }
    }
  }

  return (
    <div>
      <div className="flex items-center justify-between text-xs mb-1">
        <span className="text-secondary">Quota</span>
        <div className="flex items-center gap-2">
          {depletionText && (
            <span className={`${textColor} text-[10px]`}>{depletionText}</span>
          )}
          <span className={quota.is_depleted ? "text-red-400" : "text-secondary"}>
            {quota.is_depleted
              ? "Depleted"
              : quota.remaining != null
              ? `${quota.remaining} / ${total}`
              : "OK"}
          </span>
        </div>
      </div>
      <div className="h-2 rounded-full bg-tertiary">
        <div
          className={`h-full rounded-full transition-all ${
            quota.is_depleted ? "bg-red-500" : barColor
          }`}
          style={{
            width: `${total > 0
              ? Math.max(2, ((total - (quota.remaining ?? 0)) / total) * 100)
              : 100}%`,
          }}
        />
      </div>
      <div className="flex items-center justify-between text-[10px] text-secondary mt-0.5">
        <span>{used} used</span>
        <span>{total} total</span>
      </div>
    </div>
  );
}

function formatNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}
