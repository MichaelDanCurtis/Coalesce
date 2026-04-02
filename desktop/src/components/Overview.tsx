import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api/client";
import { useApi } from "../hooks/useApi";

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

  // Last 10 latencies for sparkline
  const latencies = recent.slice(0, 10).map((r: any) => r.latency_ms ?? 0).reverse();

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Overview</h2>

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
      // ease-out
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
