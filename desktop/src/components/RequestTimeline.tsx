import { useCallback, useEffect, useState } from "react";
import { api } from "../api/client";
import type { RequestEntry, SearchParams } from "../types";

const TIERS = ["", "SIMPLE", "MEDIUM", "COMPLEX", "REASONING"];
const PAGE_SIZE = 50;

export default function RequestTimeline() {
  const [entries, setEntries] = useState<RequestEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Filters
  const [search, setSearch] = useState("");
  const [provider, setProvider] = useState("");
  const [tier, setTier] = useState("");
  const [model, setModel] = useState("");
  const [failuresOnly, setFailuresOnly] = useState(false);
  const [page, setPage] = useState(0);

  // Known providers for dropdown
  const [providers, setProviders] = useState<string[]>([]);

  useEffect(() => {
    api.getProviders().then((data: any) => {
      const names = (data?.providers ?? []).map((p: any) => p.name || p.provider).filter(Boolean);
      setProviders(names);
    }).catch(() => {});
  }, []);

  const buildParams = useCallback((): SearchParams => ({
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
    ...(search && { search }),
    ...(provider && { provider }),
    ...(tier && { tier }),
    ...(model && { model }),
    ...(failuresOnly && { failures_only: true }),
  }), [search, provider, tier, model, failuresOnly, page]);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await api.searchRequests(buildParams());
      setEntries(result.entries);
      setTotal(result.total);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  }, [buildParams]);

  useEffect(() => { fetchData(); }, [fetchData]);

  // Reset page on filter change
  useEffect(() => { setPage(0); }, [search, provider, tier, model, failuresOnly]);

  const totalPages = Math.ceil(total / PAGE_SIZE);

  const formatTime = (ts?: number) => {
    if (!ts) return "-";
    return new Date(ts * 1000).toLocaleString(undefined, {
      month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit",
    });
  };

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-semibold">Request History</h2>
        <div className="flex gap-2">
          <a
            href={api.exportCsvUrl(buildParams())}
            target="_blank"
            rel="noreferrer"
            className="btn-ghost text-xs"
          >
            Export CSV
          </a>
          <a
            href={api.exportJsonUrl(buildParams())}
            target="_blank"
            rel="noreferrer"
            className="btn-ghost text-xs"
          >
            Export JSON
          </a>
        </div>
      </div>

      {/* Filters */}
      <div className="card">
        <div className="flex flex-wrap gap-3 items-end">
          <div className="flex-1 min-w-[200px]">
            <label className="text-xs text-secondary block mb-1">Search</label>
            <input
              type="text"
              placeholder="Search model, provider, tier..."
              className="input-field w-full"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <div>
            <label className="text-xs text-secondary block mb-1">Provider</label>
            <select
              className="input-field"
              value={provider}
              onChange={(e) => setProvider(e.target.value)}
            >
              <option value="">All Providers</option>
              {providers.map((p) => (
                <option key={p} value={p}>{p}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="text-xs text-secondary block mb-1">Tier</label>
            <select
              className="input-field"
              value={tier}
              onChange={(e) => setTier(e.target.value)}
            >
              {TIERS.map((t) => (
                <option key={t} value={t}>{t || "All Tiers"}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="text-xs text-secondary block mb-1">Model</label>
            <input
              type="text"
              placeholder="Filter model..."
              className="input-field w-32"
              value={model}
              onChange={(e) => setModel(e.target.value)}
            />
          </div>
          <label className="flex items-center gap-2 text-sm cursor-pointer pb-1">
            <input
              type="checkbox"
              checked={failuresOnly}
              onChange={(e) => setFailuresOnly(e.target.checked)}
              className="rounded"
            />
            Failures only
          </label>
        </div>
      </div>

      {/* Results info */}
      <div className="flex items-center justify-between text-xs text-secondary">
        <span>
          {total.toLocaleString()} results
          {(search || provider || tier || model || failuresOnly) ? " (filtered)" : ""}
        </span>
        {totalPages > 1 && (
          <div className="flex items-center gap-2">
            <button
              className="btn-ghost text-xs"
              disabled={page === 0}
              onClick={() => setPage(p => p - 1)}
            >
              Prev
            </button>
            <span>Page {page + 1} of {totalPages}</span>
            <button
              className="btn-ghost text-xs"
              disabled={page >= totalPages - 1}
              onClick={() => setPage(p => p + 1)}
            >
              Next
            </button>
          </div>
        )}
      </div>

      {error && (
        <div className="card border-red-900 text-red-400 text-sm">{error}</div>
      )}

      {loading ? (
        <div className="flex items-center justify-center h-32 text-secondary">Loading...</div>
      ) : entries.length === 0 ? (
        <div className="card text-center py-12 text-secondary">
          <p>No requests match your filters.</p>
        </div>
      ) : (
        <div className="card overflow-hidden p-0">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-themed text-left text-xs text-secondary">
                <th className="px-3 py-2">Time</th>
                <th className="px-3 py-2">Tier</th>
                <th className="px-3 py-2">Score</th>
                <th className="px-3 py-2">Provider</th>
                <th className="px-3 py-2">Model</th>
                <th className="px-3 py-2 text-right">In</th>
                <th className="px-3 py-2 text-right">Out</th>
                <th className="px-3 py-2 text-right">Cost</th>
                <th className="px-3 py-2 text-right">Latency</th>
                <th className="px-3 py-2">Status</th>
                <th className="px-3 py-2"></th>
              </tr>
            </thead>
            <tbody>
              {entries.map((req, i) => (
                <tr key={req.id ?? i} className="border-b border-themed-faint hover:bg-hover">
                  <td className="px-3 py-2 text-xs text-secondary whitespace-nowrap">
                    {formatTime(req.timestamp)}
                  </td>
                  <td className="px-3 py-2">
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
                  <td className="px-3 py-2 text-secondary">{req.score?.toFixed(3)}</td>
                  <td className="px-3 py-2">{req.provider}</td>
                  <td className="px-3 py-2 font-mono text-xs text-secondary max-w-[200px] truncate">
                    {req.model}
                  </td>
                  <td className="px-3 py-2 text-right text-secondary">
                    {req.input_tokens ?? "-"}
                  </td>
                  <td className="px-3 py-2 text-right text-secondary">
                    {req.output_tokens ?? "-"}
                  </td>
                  <td className="px-3 py-2 text-right">
                    <span className={req.cost_usd && req.cost_usd > 0 ? "text-amber-400" : "text-emerald-400"}>
                      ${(req.cost_usd ?? 0).toFixed(6)}
                    </span>
                  </td>
                  <td className="px-3 py-2 text-right text-secondary">
                    {req.latency_ms ?? "-"}ms
                  </td>
                  <td className="px-3 py-2">
                    {req.success ? (
                      <span className="badge-green">OK</span>
                    ) : (
                      <span className="badge-red">FAIL</span>
                    )}
                  </td>
                  <td className="px-3 py-2 text-right">
                    <button
                      className="text-xs text-secondary hover:text-primary"
                      title="Open a new chat with this model"
                      onClick={() => {
                        const payload = { prompt: "", model: req.model };
                        localStorage.setItem(
                          "coalesce_chat_handoff",
                          JSON.stringify(payload),
                        );
                        window.dispatchEvent(new CustomEvent("coalesce:chat-handoff"));
                        window.dispatchEvent(
                          new CustomEvent("coalesce:set-tab", { detail: { tab: "chat" } }),
                        );
                      }}
                    >
                      ↗ Chat
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Bottom pagination */}
      {totalPages > 1 && (
        <div className="flex justify-center gap-2">
          <button
            className="btn-ghost text-xs"
            disabled={page === 0}
            onClick={() => setPage(p => p - 1)}
          >
            Previous
          </button>
          <span className="text-sm text-secondary py-1">Page {page + 1} of {totalPages}</span>
          <button
            className="btn-ghost text-xs"
            disabled={page >= totalPages - 1}
            onClick={() => setPage(p => p + 1)}
          >
            Next
          </button>
        </div>
      )}
    </div>
  );
}
