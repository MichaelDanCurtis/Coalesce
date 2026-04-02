import { useCallback } from "react";
import { api } from "../api/client";
import { useApi } from "../hooks/useApi";

export default function CostAnalytics() {
  const fetchCosts = useCallback(() => api.getCosts(30), []);
  const { data, loading, error } = useApi(fetchCosts);

  if (loading) {
    return <div className="flex items-center justify-center h-64 text-secondary">Loading...</div>;
  }
  if (error) {
    return (
      <div className="space-y-6">
        <h2 className="text-xl font-semibold">Cost Analytics</h2>
        <div className="card text-center py-12">
          <p className="text-red-400 text-lg font-medium">Proxy Offline</p>
          <p className="text-secondary mt-2 text-sm">
            Start the proxy with: <code className="bg-tertiary px-2 py-1 rounded">coalesce serve</code>
          </p>
        </div>
      </div>
    );
  }

  const costs = data as any;
  const summary = costs?.summary ?? {};
  const byProvider = costs?.by_provider ?? [];
  const byModel = costs?.by_model ?? [];
  const daily = costs?.daily ?? [];
  const budget = costs?.budget ?? {};

  const budgetPct = budget.total_limit_usd > 0
    ? ((summary.total_cost_usd / budget.total_limit_usd) * 100).toFixed(1)
    : null;

  // Savings waterfall calculations
  const totalRequests = summary.total_requests ?? 0;
  const freeRequests = summary.total_free_requests ?? 0;
  const estimatedPerRequestCost = 0.003;
  const wouldHavePaid = totalRequests * estimatedPerRequestCost;
  const freeSavings = freeRequests * estimatedPerRequestCost;
  const actualPaid = summary.total_cost_usd ?? 0;
  const quotaSavings = Math.max(0, wouldHavePaid - freeSavings - actualPaid);

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Cost Analytics</h2>

      {/* Summary Cards */}
      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <SummaryCard label="Total Spent" value={`$${summary.total_cost_usd?.toFixed(4) ?? "0"}`}
          color={summary.total_cost_usd > 0 ? "text-amber-400" : "text-emerald-400"} />
        <SummaryCard label="Total Tokens" value={formatNumber(
          (summary.total_input_tokens ?? 0) + (summary.total_output_tokens ?? 0)
        )} />
        <SummaryCard label="Free Requests" value={summary.total_free_requests ?? 0}
          subtitle={summary.total_requests > 0
            ? `${((summary.total_free_requests / summary.total_requests) * 100).toFixed(0)}% of total`
            : undefined}
          color="text-emerald-400" />
        <SummaryCard label="Avg Latency" value={`${(summary.avg_latency_ms ?? 0).toFixed(0)}ms`} />
      </div>

      {/* Budget Bar */}
      {budget.total_limit_usd > 0 && (
        <div className="card">
          <div className="flex items-center justify-between mb-2">
            <h3 className="text-sm font-medium">Budget Usage</h3>
            <span className="text-xs text-secondary">
              ${summary.total_cost_usd?.toFixed(4)} / ${budget.total_limit_usd?.toFixed(2)}
              {budgetPct && ` (${budgetPct}%)`}
            </span>
          </div>
          <div className="h-2 rounded-full bg-tertiary">
            <div
              className={`h-full rounded-full transition-all ${
                Number(budgetPct) > 80 ? "bg-red-500" : Number(budgetPct) > 50 ? "bg-amber-500" : "bg-emerald-500"
              }`}
              style={{ width: `${Math.min(100, Number(budgetPct) || 0)}%` }}
            />
          </div>
        </div>
      )}

      {/* Savings Waterfall */}
      {totalRequests > 0 && (
        <div className="card">
          <h3 className="text-sm font-medium mb-3">Savings Waterfall</h3>
          <div className="space-y-2">
            {/* Stacked horizontal bar */}
            <div className="h-6 rounded-full bg-tertiary flex overflow-hidden">
              {wouldHavePaid > 0 && (
                <>
                  <div
                    className="h-full bg-emerald-500 transition-all"
                    style={{ width: `${(freeSavings / wouldHavePaid) * 100}%` }}
                    title={`Free Savings: $${freeSavings.toFixed(4)}`}
                  />
                  <div
                    className="h-full bg-sky-500 transition-all"
                    style={{ width: `${(quotaSavings / wouldHavePaid) * 100}%` }}
                    title={`Quota Savings: $${quotaSavings.toFixed(4)}`}
                  />
                  <div
                    className="h-full bg-amber-500 transition-all"
                    style={{ width: `${(actualPaid / wouldHavePaid) * 100}%` }}
                    title={`Actual Paid: $${actualPaid.toFixed(4)}`}
                  />
                </>
              )}
            </div>
            {/* Legend */}
            <div className="flex flex-wrap gap-4 text-xs">
              <div className="flex items-center gap-1.5">
                <div className="h-2.5 w-2.5 rounded bg-gray-500" />
                <span className="text-secondary">Would Have Paid: ${wouldHavePaid.toFixed(4)}</span>
              </div>
              <div className="flex items-center gap-1.5">
                <div className="h-2.5 w-2.5 rounded bg-emerald-500" />
                <span className="text-secondary">Free Savings: ${freeSavings.toFixed(4)}</span>
              </div>
              <div className="flex items-center gap-1.5">
                <div className="h-2.5 w-2.5 rounded bg-sky-500" />
                <span className="text-secondary">Quota Savings: ${quotaSavings.toFixed(4)}</span>
              </div>
              <div className="flex items-center gap-1.5">
                <div className="h-2.5 w-2.5 rounded bg-amber-500" />
                <span className="text-secondary">Actual Paid: ${actualPaid.toFixed(4)}</span>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Daily Trend */}
      {daily.length > 0 && (
        <div className="card">
          <h3 className="text-sm font-medium mb-3">Daily Cost Trend (30d)</h3>
          <div className="flex items-end gap-1 h-32">
            {daily.map((d: any, i: number) => {
              const maxCost = Math.max(...daily.map((x: any) => x.total_cost_usd), 0.0001);
              const height = (d.total_cost_usd / maxCost) * 100;
              const freeRatio = d.requests > 0 ? d.free_requests / d.requests : 1;
              return (
                <div key={i} className="flex-1 flex flex-col items-center gap-0.5 group relative">
                  <div className="w-full rounded-t" style={{ height: `${Math.max(2, height)}%` }}>
                    <div className="w-full h-full rounded-t bg-brand-500/60" />
                  </div>
                  {/* Tooltip */}
                  <div className="absolute bottom-full mb-2 hidden group-hover:block z-10">
                    <div className="card text-xs whitespace-nowrap p-2 shadow-xl">
                      <p className="font-medium">{d.date}</p>
                      <p className="text-secondary">{d.requests} req · ${d.total_cost_usd.toFixed(4)}</p>
                      <p className="text-emerald-400">{(freeRatio * 100).toFixed(0)}% free</p>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Cost by Provider */}
      {byProvider.length > 0 && (
        <div>
          <h3 className="text-sm font-medium text-secondary mb-3">Cost by Provider</h3>
          <div className="card overflow-hidden p-0">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-themed text-left text-xs text-secondary">
                  <th className="px-4 py-2">Provider</th>
                  <th className="px-4 py-2 text-right">Requests</th>
                  <th className="px-4 py-2 text-right">Input Tokens</th>
                  <th className="px-4 py-2 text-right">Output Tokens</th>
                  <th className="px-4 py-2 text-right">Total Cost</th>
                  <th className="px-4 py-2 text-right">Avg Latency</th>
                </tr>
              </thead>
              <tbody>
                {byProvider.map((p: any) => (
                  <tr key={p.provider} className="border-b border-themed-faint hover:bg-hover">
                    <td className="px-4 py-2 font-medium">{p.provider}</td>
                    <td className="px-4 py-2 text-right text-secondary">{p.requests}</td>
                    <td className="px-4 py-2 text-right text-secondary">{formatNumber(p.input_tokens)}</td>
                    <td className="px-4 py-2 text-right text-secondary">{formatNumber(p.output_tokens)}</td>
                    <td className="px-4 py-2 text-right">
                      <span className={p.total_cost_usd > 0 ? "text-amber-400" : "text-emerald-400"}>
                        ${p.total_cost_usd.toFixed(6)}
                      </span>
                    </td>
                    <td className="px-4 py-2 text-right text-secondary">{p.avg_latency_ms.toFixed(0)}ms</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Cost by Model */}
      {byModel.length > 0 && (
        <div>
          <h3 className="text-sm font-medium text-secondary mb-3">Cost by Model</h3>
          <div className="card overflow-hidden p-0">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-themed text-left text-xs text-secondary">
                  <th className="px-4 py-2">Model</th>
                  <th className="px-4 py-2">Provider</th>
                  <th className="px-4 py-2 text-right">Requests</th>
                  <th className="px-4 py-2 text-right">Tokens (in/out)</th>
                  <th className="px-4 py-2 text-right">Total Cost</th>
                </tr>
              </thead>
              <tbody>
                {byModel.slice(0, 20).map((m: any, i: number) => (
                  <tr key={i} className="border-b border-themed-faint hover:bg-hover">
                    <td className="px-4 py-2 font-mono text-xs">{m.model}</td>
                    <td className="px-4 py-2 text-secondary">{m.provider}</td>
                    <td className="px-4 py-2 text-right text-secondary">{m.requests}</td>
                    <td className="px-4 py-2 text-right text-secondary">
                      {formatNumber(m.input_tokens)} / {formatNumber(m.output_tokens)}
                    </td>
                    <td className="px-4 py-2 text-right">
                      <span className={m.total_cost_usd > 0 ? "text-amber-400" : "text-emerald-400"}>
                        ${m.total_cost_usd.toFixed(6)}
                      </span>
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

function SummaryCard({
  label,
  value,
  subtitle,
  color,
}: {
  label: string;
  value: string | number;
  subtitle?: string;
  color?: string;
}) {
  return (
    <div className="card">
      <p className="text-xs text-secondary">{label}</p>
      <p className={`mt-1 text-2xl font-semibold ${color ?? ""}`}>{value}</p>
      {subtitle && <p className="text-xs text-secondary mt-1">{subtitle}</p>}
    </div>
  );
}

function formatNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}
