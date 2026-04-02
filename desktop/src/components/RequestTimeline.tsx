import { useCallback } from "react";
import { api } from "../api/client";
import { useApi } from "../hooks/useApi";

export default function RequestTimeline() {
  const fetchTimeline = useCallback(() => api.getStats(), []);
  const { data, loading, error } = useApi(fetchTimeline);

  if (loading) {
    return <div className="flex items-center justify-center h-64 text-secondary">Loading...</div>;
  }
  if (error) {
    return <div className="card border-red-900 text-red-400 text-sm">{error}</div>;
  }

  const requests = (data as any)?.recent_requests ?? [];

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Request Timeline</h2>

      {requests.length === 0 ? (
        <div className="card text-center py-12 text-secondary">
          <p>No requests yet. Send a request through the proxy to see it here.</p>
        </div>
      ) : (
        <div className="card overflow-hidden p-0">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-themed text-left text-xs text-secondary">
                <th className="px-4 py-2">Tier</th>
                <th className="px-4 py-2">Score</th>
                <th className="px-4 py-2">Provider</th>
                <th className="px-4 py-2">Model</th>
                <th className="px-4 py-2 text-right">In Tokens</th>
                <th className="px-4 py-2 text-right">Out Tokens</th>
                <th className="px-4 py-2 text-right">Cost</th>
                <th className="px-4 py-2 text-right">Latency</th>
                <th className="px-4 py-2">Status</th>
              </tr>
            </thead>
            <tbody>
              {requests.map((req: any, i: number) => (
                <tr key={i} className="border-b border-themed-faint hover:bg-hover">
                  <td className="px-4 py-2">
                    <span
                      className={
                        req.tier === "SIMPLE"
                          ? "badge-green"
                          : req.tier === "REASONING"
                          ? "badge-red"
                          : "badge-yellow"
                      }
                    >
                      {req.tier}
                    </span>
                  </td>
                  <td className="px-4 py-2 text-secondary">{req.score?.toFixed(3)}</td>
                  <td className="px-4 py-2">{req.provider}</td>
                  <td className="px-4 py-2 font-mono text-xs text-secondary">{req.model}</td>
                  <td className="px-4 py-2 text-right text-secondary">
                    {req.input_tokens ?? "-"}
                  </td>
                  <td className="px-4 py-2 text-right text-secondary">
                    {req.output_tokens ?? "-"}
                  </td>
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
      )}
    </div>
  );
}
