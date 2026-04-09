import { useState, useCallback, useEffect } from "react";
import { api } from "../api/client";
import { useTranslation } from "../i18n";

interface OllamaModel {
  name: string;
  size_bytes: number;
  parameter_size: string;
  family: string;
  enabled: boolean;
}

interface RunningModel {
  name: string;
  size_bytes: number;
  size_vram: number;
  gpu_percent: number;
  processor: string;
  parameter_size: string;
  family: string;
  quantization: string;
  expires_at: string;
}

interface LibraryModel {
  name: string;
  label: string;
  sizes: string;
  description: string;
  source?: string;
  ram_estimate_gb?: number;
  downloads?: number;
  likes?: number;
  pulls?: string;
  updated?: string;
}

interface BenchmarkResult {
  model: string;
  tokens_generated: number;
  generation_tokens_per_sec: number;
  prompt_tokens: number;
  prompt_tokens_per_sec: number;
  total_duration_ms: number;
}

interface GpuInfo {
  gpus: Array<Record<string, unknown>>;
  acceleration: string;
}

interface ResourceData {
  models: Array<{ name: string; ram_bytes: number; vram_bytes: number }>;
  total_model_ram: number;
  total_model_vram: number;
  system: { total_bytes?: number; used_bytes?: number; free_bytes?: number };
}

export default function LocalLLM() {
  const { t } = useTranslation();

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">{t("localllm.title")}</h2>
      <OllamaStatusBar />
      <GpuInfoPanel />
      <ResourceMonitor />
      <RunningModels />
      <PullModel />
      <ModelBrowser />
      <InstalledModels />
      <ImportGGUF />
    </div>
  );
}

/* ─── Ollama Status Bar (Start/Stop) ─── */

function OllamaStatusBar() {
  const { t } = useTranslation();
  const [running, setRunning] = useState<boolean | null>(null);
  const [version, setVersion] = useState("");
  const [loading, setLoading] = useState(false);

  const checkStatus = useCallback(async () => {
    try {
      const data = await api.ollamaStatus();
      setRunning(data.running);
      setVersion(data.version || "");
    } catch {
      setRunning(false);
    }
  }, []);

  useEffect(() => {
    checkStatus();
    const interval = setInterval(checkStatus, 10000);
    return () => clearInterval(interval);
  }, [checkStatus]);

  const start = async () => {
    setLoading(true);
    try {
      const result = await api.ollamaStart();
      if (result.status === "started" || result.status === "already_running") {
        setRunning(true);
      }
    } catch {}
    setLoading(false);
    checkStatus();
  };

  const stop = async () => {
    setLoading(true);
    try {
      const result = await api.ollamaStop();
      if (result.status === "stopped") {
        setRunning(false);
      }
    } catch {}
    setLoading(false);
    checkStatus();
  };

  return (
    <div className="card flex items-center justify-between">
      <div className="flex items-center gap-3">
        <div className={`h-3 w-3 rounded-full ${
          running === null ? "bg-zinc-500 animate-pulse" :
          running ? "bg-emerald-500" : "bg-red-500"
        }`} />
        <div>
          <span className="font-medium">Ollama</span>
          {running && version && <span className="text-xs text-secondary ml-2">v{version}</span>}
          <span className="text-xs text-secondary ml-2">
            {running === null ? "Checking..." : running ? t("localllm.status_running") : t("localllm.status_stopped")}
          </span>
        </div>
      </div>
      <div className="flex gap-2">
        {running ? (
          <button onClick={stop} disabled={loading} className="btn-ghost text-xs text-red-400 hover:text-red-300">
            {loading ? "Stopping..." : t("localllm.stop")}
          </button>
        ) : (
          <button onClick={start} disabled={loading || running === null} className="btn-primary text-sm px-4">
            {loading ? "Starting..." : t("localllm.start")}
          </button>
        )}
      </div>
    </div>
  );
}

/* ─── GPU Hardware Info ─── */

function GpuInfoPanel() {
  const { t } = useTranslation();
  const [gpu, setGpu] = useState<GpuInfo | null>(null);

  useEffect(() => {
    api.ollamaStatus().then(data => setGpu(data.gpu)).catch(() => {});
  }, []);

  if (!gpu || gpu.gpus.length === 0) return null;

  return (
    <div>
      <h3 className="text-sm font-medium text-secondary mb-3">{t("localllm.gpu_info")}</h3>
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
        {gpu.gpus.map((g, i) => (
          <div key={i} className="card">
            <div className="flex items-center justify-between mb-2">
              <span className="font-medium text-sm">{String(g.name || "GPU")}</span>
              <span className={`text-xs px-2 py-0.5 rounded-full font-medium ${
                gpu.acceleration === "Metal" ? "bg-blue-500/20 text-blue-400" :
                gpu.acceleration === "CUDA" ? "bg-emerald-500/20 text-emerald-400" :
                "bg-zinc-500/20 text-zinc-400"
              }`}>
                {gpu.acceleration}
              </span>
            </div>
            <div className="grid grid-cols-2 gap-2 text-xs text-secondary">
              {"vram" in g && g.vram ? <span>VRAM: {String(g.vram)}</span> : null}
              {"cores" in g && g.cores ? <span>Cores: {String(g.cores)}</span> : null}
              {"metal" in g && g.metal ? <span>Metal: {String(g.metal)}</span> : null}
              {"vram_total_mb" in g && g.vram_total_mb ? <span>VRAM: {String(g.vram_total_mb)} MB total</span> : null}
              {"vram_used_mb" in g && g.vram_used_mb ? <span>Used: {String(g.vram_used_mb)} MB</span> : null}
              {"vram_free_mb" in g && g.vram_free_mb ? <span>Free: {String(g.vram_free_mb)} MB</span> : null}
              {"utilization" in g ? <span>Util: {String(g.utilization)}%</span> : null}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

/* ─── Resource Monitor ─── */

function ResourceMonitor() {
  const { t } = useTranslation();
  const [resources, setResources] = useState<ResourceData | null>(null);
  const [history, setHistory] = useState<Array<{ ram: number; vram: number }>>([]);

  useEffect(() => {
    const fetchResources = async () => {
      try {
        const data = await api.ollamaResources();
        setResources(data);
        setHistory(prev => {
          const next = [...prev, { ram: data.total_model_ram, vram: data.total_model_vram }];
          return next.slice(-30); // Keep last 30 data points
        });
      } catch {}
    };
    fetchResources();
    const interval = setInterval(fetchResources, 5000);
    return () => clearInterval(interval);
  }, []);

  if (!resources) return null;

  const sys = resources.system;
  const totalMem = sys.total_bytes || 1;
  const usedMem = sys.used_bytes || 0;
  const memPercent = Math.round((usedMem / totalMem) * 100);

  return (
    <div>
      <h3 className="text-sm font-medium text-secondary mb-3">{t("localllm.resources")}</h3>
      <div className="card">
        {/* System Memory Bar */}
        <div className="mb-3">
          <div className="flex justify-between text-xs text-secondary mb-1">
            <span>{t("localllm.system_memory")}</span>
            <span>{formatSize(usedMem)} / {formatSize(totalMem)} ({memPercent}%)</span>
          </div>
          <div className="h-3 rounded-full bg-tertiary overflow-hidden flex">
            {/* Model RAM portion */}
            <div
              className="h-full bg-blue-500"
              style={{ width: `${(resources.total_model_ram / totalMem) * 100}%` }}
              title={`Models (RAM): ${formatSize(resources.total_model_ram)}`}
            />
            {/* Model VRAM portion */}
            <div
              className="h-full bg-emerald-500"
              style={{ width: `${(resources.total_model_vram / totalMem) * 100}%` }}
              title={`Models (VRAM): ${formatSize(resources.total_model_vram)}`}
            />
            {/* Other system usage */}
            <div
              className="h-full bg-zinc-500"
              style={{ width: `${Math.max(0, memPercent - ((resources.total_model_ram + resources.total_model_vram) / totalMem) * 100)}%` }}
            />
          </div>
          <div className="flex gap-4 mt-1 text-xs text-secondary">
            <span className="flex items-center gap-1"><span className="h-2 w-2 rounded-full bg-blue-500 inline-block" /> Model RAM: {formatSize(resources.total_model_ram)}</span>
            <span className="flex items-center gap-1"><span className="h-2 w-2 rounded-full bg-emerald-500 inline-block" /> Model VRAM: {formatSize(resources.total_model_vram)}</span>
          </div>
        </div>

        {/* Mini Sparkline */}
        {history.length > 1 && (
          <div className="mt-2">
            <p className="text-xs text-secondary mb-1">Memory trend (last {history.length * 5}s)</p>
            <svg viewBox={`0 0 ${history.length * 4} 30`} className="w-full h-6" preserveAspectRatio="none">
              <polyline
                fill="none"
                stroke="var(--color-brand-500, #6366f1)"
                strokeWidth="1.5"
                points={history.map((h, i) => `${i * 4},${30 - (h.ram / totalMem) * 30}`).join(" ")}
              />
              <polyline
                fill="none"
                stroke="#10b981"
                strokeWidth="1.5"
                points={history.map((h, i) => `${i * 4},${30 - (h.vram / totalMem) * 30}`).join(" ")}
              />
            </svg>
          </div>
        )}

        {/* Per-model breakdown */}
        {resources.models.length > 0 && (
          <div className="mt-3 space-y-1">
            {resources.models.map(m => (
              <div key={m.name} className="flex items-center justify-between text-xs">
                <span className="font-mono">{m.name}</span>
                <span className="text-secondary">
                  {formatSize(m.ram_bytes)} RAM / {formatSize(m.vram_bytes)} VRAM
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/* ─── Running Models (GPU Status) ─── */

function RunningModels() {
  const { t } = useTranslation();
  const [models, setModels] = useState<RunningModel[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [keepAliveModel, setKeepAliveModel] = useState<string | null>(null);
  const [keepAliveVal, setKeepAliveVal] = useState("5m");
  const [benchmarking, setBenchmarking] = useState<string | null>(null);
  const [benchResult, setBenchResult] = useState<BenchmarkResult | null>(null);
  const [gpuLayerModel, setGpuLayerModel] = useState<string | null>(null);
  const [gpuLayers, setGpuLayers] = useState(-1);

  const refresh = useCallback(async () => {
    try {
      const data = await api.ollamaRunning();
      setModels(data.models ?? []);
      setError("");
    } catch {
      setError("Cannot connect to Ollama");
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, [refresh]);

  const setKeepAlive = async (model: string) => {
    try {
      await api.ollamaKeepAlive(model, keepAliveVal);
      setKeepAliveModel(null);
    } catch {}
  };

  const benchmark = async (model: string) => {
    setBenchmarking(model);
    setBenchResult(null);
    try {
      const result = await api.ollamaBenchmark(model);
      setBenchResult(result);
    } catch {}
    setBenchmarking(null);
  };

  const setLayers = async (model: string) => {
    try {
      await api.ollamaGpuLayers(model, gpuLayers);
      setGpuLayerModel(null);
      refresh();
    } catch {}
  };

  return (
    <div>
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-sm font-medium text-secondary">{t("localllm.running")}</h3>
        <button onClick={refresh} className="btn-ghost text-xs">{t("localllm.refresh")}</button>
      </div>

      {error ? (
        <div className="card text-center py-6 text-secondary text-sm">Ollama not reachable</div>
      ) : loading ? (
        <div className="card text-center py-6 text-secondary text-sm">Loading...</div>
      ) : models.length === 0 ? (
        <div className="card text-center py-6 text-secondary text-sm">{t("localllm.no_running")}</div>
      ) : (
        <div className="space-y-3">
          {models.map(m => (
            <div key={m.name} className="card">
              <div className="flex items-center justify-between mb-2">
                <span className="font-mono text-sm font-medium">{m.name}</span>
                <div className="flex items-center gap-2">
                  <span className={`text-xs px-2 py-0.5 rounded-full font-medium ${
                    m.processor === "GPU" ? "bg-emerald-500/20 text-emerald-400" :
                    m.processor === "GPU/CPU" ? "bg-amber-500/20 text-amber-400" :
                    "bg-zinc-500/20 text-zinc-400"
                  }`}>
                    {m.processor}
                  </span>
                  <span className="text-xs text-secondary">{m.quantization || ""}</span>
                </div>
              </div>

              {/* VRAM Bar */}
              <div className="mb-2">
                <div className="flex justify-between text-xs text-secondary mb-1">
                  <span>VRAM: {formatSize(m.size_vram)} / {formatSize(m.size_bytes)}</span>
                  <span>{m.gpu_percent}% GPU</span>
                </div>
                <div className="h-2 rounded-full bg-tertiary overflow-hidden">
                  <div
                    className={`h-full rounded-full transition-all ${
                      m.gpu_percent > 90 ? "bg-emerald-500" :
                      m.gpu_percent > 0 ? "bg-amber-500" : "bg-zinc-500"
                    }`}
                    style={{ width: `${Math.max(m.gpu_percent, 2)}%` }}
                  />
                </div>
              </div>

              <div className="flex items-center gap-3 text-xs text-secondary">
                <span>{m.family || "—"} · {m.parameter_size || "—"}</span>
              </div>

              {/* Action buttons */}
              <div className="flex gap-2 mt-3 flex-wrap">
                <button
                  onClick={() => benchmark(m.name)}
                  disabled={benchmarking !== null}
                  className="btn-ghost text-xs"
                >
                  {benchmarking === m.name ? "Running..." : "Benchmark"}
                </button>
                <button
                  onClick={() => setKeepAliveModel(keepAliveModel === m.name ? null : m.name)}
                  className="btn-ghost text-xs"
                >
                  Keep-Alive
                </button>
                <button
                  onClick={() => setGpuLayerModel(gpuLayerModel === m.name ? null : m.name)}
                  className="btn-ghost text-xs"
                >
                  GPU Layers
                </button>
                <button
                  onClick={() => api.ollamaKeepAlive(m.name, "0").then(refresh)}
                  className="btn-ghost text-xs text-amber-400 hover:text-amber-300"
                >
                  Unload
                </button>
              </div>

              {/* Keep-Alive Popover */}
              {keepAliveModel === m.name && (
                <div className="mt-2 flex items-center gap-2 p-2 rounded-lg bg-tertiary">
                  <select
                    value={keepAliveVal}
                    onChange={e => setKeepAliveVal(e.target.value)}
                    className="rounded border border-themed bg-surface px-2 py-1 text-xs"
                  >
                    <option value="0">Unload now</option>
                    <option value="5m">5 minutes</option>
                    <option value="15m">15 minutes</option>
                    <option value="30m">30 minutes</option>
                    <option value="1h">1 hour</option>
                    <option value="4h">4 hours</option>
                    <option value="24h">24 hours</option>
                    <option value="-1">Forever</option>
                  </select>
                  <button onClick={() => setKeepAlive(m.name)} className="btn-primary text-xs px-3">Set</button>
                </div>
              )}

              {/* GPU Layer Slider */}
              {gpuLayerModel === m.name && (
                <div className="mt-2 p-2 rounded-lg bg-tertiary">
                  <div className="flex items-center gap-3">
                    <label className="text-xs text-secondary whitespace-nowrap">GPU Layers:</label>
                    <input
                      type="range"
                      min={-1}
                      max={100}
                      value={gpuLayers}
                      onChange={e => setGpuLayers(parseInt(e.target.value))}
                      className="flex-1"
                    />
                    <span className="text-xs font-mono w-8 text-right">
                      {gpuLayers === -1 ? "All" : gpuLayers === 0 ? "CPU" : gpuLayers}
                    </span>
                    <button onClick={() => setLayers(m.name)} className="btn-primary text-xs px-3">Apply</button>
                  </div>
                  <p className="text-xs text-secondary mt-1">
                    -1 = all layers on GPU, 0 = CPU only, 1-100 = specific layer count
                  </p>
                </div>
              )}

              {/* Benchmark Result */}
              {benchResult && benchResult.model === m.name && (
                <div className="mt-2 p-2 rounded-lg bg-tertiary">
                  <div className="grid grid-cols-3 gap-3 text-xs">
                    <div className="text-center">
                      <p className="text-lg font-bold text-brand-400">{benchResult.generation_tokens_per_sec}</p>
                      <p className="text-secondary">tok/s (gen)</p>
                    </div>
                    <div className="text-center">
                      <p className="text-lg font-bold text-emerald-400">{benchResult.prompt_tokens_per_sec}</p>
                      <p className="text-secondary">tok/s (prompt)</p>
                    </div>
                    <div className="text-center">
                      <p className="text-lg font-bold">{(benchResult.total_duration_ms / 1000).toFixed(1)}s</p>
                      <p className="text-secondary">total time</p>
                    </div>
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/* ─── Pull New Model (with Quantization Picker) ─── */

function PullModel() {
  const { t } = useTranslation();
  const [modelName, setModelName] = useState("");
  const [tags, setTags] = useState<string[]>([]);
  const [selectedTag, setSelectedTag] = useState("");
  const [loadingTags, setLoadingTags] = useState(false);

  // Pull queue
  const [queue, setQueue] = useState<Array<{ name: string; status: string; percent: number | null; error?: string }>>([]);
  const [pulling, setPulling] = useState(false);

  // When model name changes, fetch available tags
  useEffect(() => {
    if (!modelName.trim() || modelName.includes(":")) {
      setTags([]);
      return;
    }
    const timeout = setTimeout(async () => {
      setLoadingTags(true);
      try {
        const data = await api.ollamaLibraryTags(modelName.trim());
        const tagNames = (data.tags || []).map((t: unknown) => {
          if (typeof t === "string") return t;
          if (typeof t === "object" && t !== null && "name" in t) return String((t as Record<string, unknown>).name);
          return "";
        }).filter(Boolean) as string[];
        setTags(tagNames.slice(0, 20));
      } catch {
        setTags([]);
      }
      setLoadingTags(false);
    }, 500);
    return () => clearTimeout(timeout);
  }, [modelName]);

  const addToQueue = () => {
    const fullName = selectedTag ? `${modelName.trim()}:${selectedTag}` : modelName.trim();
    if (!fullName) return;
    setQueue(prev => [...prev, { name: fullName, status: "queued", percent: null }]);
    setModelName("");
    setSelectedTag("");
    setTags([]);
  };

  // Process queue
  useEffect(() => {
    if (pulling) return;
    const next = queue.findIndex(q => q.status === "queued");
    if (next === -1) return;

    setPulling(true);
    const model = queue[next].name;
    setQueue(prev => prev.map((q, i) => i === next ? { ...q, status: "pulling" } : q));

    (async () => {
      try {
        const resp = await api.ollamaPull(model);
        if (!resp.ok) {
          setQueue(prev => prev.map((q, i) => i === next ? { ...q, status: "error", error: `HTTP ${resp.status}` } : q));
          setPulling(false);
          return;
        }

        const reader = resp.body?.getReader();
        if (!reader) {
          setQueue(prev => prev.map((q, i) => i === next ? { ...q, status: "error", error: "No stream" } : q));
          setPulling(false);
          return;
        }

        const decoder = new TextDecoder();
        let buffer = "";

        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split("\n");
          buffer = lines.pop() || "";
          for (const line of lines) {
            const trimmed = line.replace(/^data:\s*/, "").trim();
            if (!trimmed) continue;
            try {
              const parsed = JSON.parse(trimmed);
              const percent = parsed.total && parsed.completed
                ? Math.round((parsed.completed / parsed.total) * 100) : null;
              setQueue(prev => prev.map((q, i) =>
                i === next ? { ...q, status: parsed.status || "pulling", percent } : q
              ));
            } catch {}
          }
        }
        setQueue(prev => prev.map((q, i) => i === next ? { ...q, status: "success", percent: 100 } : q));
      } catch (e: unknown) {
        const msg = e instanceof Error ? e.message : "Failed";
        setQueue(prev => prev.map((q, i) => i === next ? { ...q, status: "error", error: msg } : q));
      }
      setPulling(false);
    })();
  }, [queue, pulling]);

  return (
    <div>
      <h3 className="text-sm font-medium text-secondary mb-3">{t("localllm.pull")}</h3>
      <div className="card">
        <div className="flex gap-2 mb-2">
          <input
            value={modelName}
            onChange={e => setModelName(e.target.value)}
            onKeyDown={e => e.key === "Enter" && addToQueue()}
            placeholder="e.g. llama3.2, phi4-mini, qwen2.5, deepseek-r1"
            className="flex-1 rounded-lg border border-themed bg-tertiary px-3 py-2 text-sm focus:border-brand-500 focus:outline-none"
          />
          {tags.length > 0 && (
            <select
              value={selectedTag}
              onChange={e => setSelectedTag(e.target.value)}
              className="rounded-lg border border-themed bg-tertiary px-2 py-2 text-sm"
            >
              <option value="">latest</option>
              {tags.map(tag => (
                <option key={tag} value={tag}>{tag}</option>
              ))}
            </select>
          )}
          {loadingTags && <span className="text-xs text-secondary self-center">...</span>}
          <button
            onClick={addToQueue}
            disabled={!modelName.trim()}
            className="btn-primary text-sm px-4"
          >
            {t("localllm.pull_btn")}
          </button>
        </div>

        <p className="text-xs text-secondary mb-3">{t("localllm.pull_hint")}</p>

        {/* Download Queue */}
        {queue.length > 0 && (
          <div className="space-y-2">
            {queue.map((item, i) => (
              <div key={`${item.name}-${i}`} className="flex items-center gap-3">
                <span className="font-mono text-xs flex-shrink-0">{item.name}</span>
                <div className="flex-1">
                  {item.percent !== null && item.status !== "success" && item.status !== "error" && (
                    <div className="h-1.5 rounded-full bg-tertiary overflow-hidden">
                      <div
                        className="h-full rounded-full bg-brand-500 transition-all duration-300"
                        style={{ width: `${item.percent}%` }}
                      />
                    </div>
                  )}
                </div>
                <span className={`text-xs ${
                  item.status === "success" ? "text-emerald-400" :
                  item.status === "error" ? "text-red-400" :
                  item.status === "queued" ? "text-zinc-400" :
                  "text-brand-400"
                }`}>
                  {item.status === "success" ? "Done" :
                   item.status === "error" ? item.error :
                   item.status === "queued" ? "Queued" :
                   item.percent !== null ? `${item.percent}%` : item.status}
                </span>
                {(item.status === "success" || item.status === "error") && (
                  <button
                    onClick={() => setQueue(prev => prev.filter((_, j) => j !== i))}
                    className="text-xs text-secondary hover:text-primary"
                  >
                    ×
                  </button>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/* ─── Model Browser (Library Search) ─── */

function ModelBrowser() {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [ollamaModels, setOllamaModels] = useState<LibraryModel[]>([]);
  const [hfModels, setHfModels] = useState<LibraryModel[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [source, setSource] = useState<"ollama" | "huggingface">("ollama");
  const [ramFilter, setRamFilter] = useState(false);
  const [systemRam, setSystemRam] = useState<number | null>(null);
  const [loadingOllama, setLoadingOllama] = useState(false);
  const [loadingHf, setLoadingHf] = useState(false);

  // Fetch system RAM once
  useEffect(() => {
    api.ollamaResources().then(data => {
      if (data.system?.total_bytes) {
        setSystemRam(data.system.total_bytes / (1024 * 1024 * 1024));
      }
    }).catch(() => {});
  }, []);

  // Search Ollama
  useEffect(() => {
    const timeout = setTimeout(async () => {
      setLoadingOllama(true);
      try {
        const data = await api.ollamaLibrarySearch(query || undefined);
        setOllamaModels(data.models ?? []);
      } catch {}
      setLoadingOllama(false);
    }, 300);
    return () => clearTimeout(timeout);
  }, [query]);

  // Search HuggingFace (only when tab active + query non-empty)
  useEffect(() => {
    if (source !== "huggingface" || !query.trim()) {
      setHfModels([]);
      return;
    }
    const timeout = setTimeout(async () => {
      setLoadingHf(true);
      try {
        const data = await api.huggingfaceSearch(query);
        setHfModels(data.models ?? []);
      } catch {}
      setLoadingHf(false);
    }, 400);
    return () => clearTimeout(timeout);
  }, [query, source]);

  const models = source === "ollama" ? ollamaModels : hfModels;
  const loading = source === "ollama" ? loadingOllama : loadingHf;

  // Apply RAM filter
  const filtered = ramFilter && systemRam
    ? models.filter(m => !m.ram_estimate_gb || m.ram_estimate_gb <= systemRam)
    : models;

  const fitsInRam = (m: LibraryModel) => {
    if (!m.ram_estimate_gb || !systemRam) return true;
    return m.ram_estimate_gb <= systemRam;
  };

  const formatNumber = (n: number) => {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
    return String(n);
  };

  return (
    <div>
      <div
        className="flex items-center justify-between cursor-pointer mb-3"
        onClick={() => setExpanded(!expanded)}
      >
        <h3 className="text-sm font-medium text-secondary">{t("localllm.browse")}</h3>
        <span className="text-xs text-secondary">{expanded ? "▲" : "▼"}</span>
      </div>

      {expanded && (
        <div className="card">
          {/* Source tabs */}
          <div className="flex items-center gap-2 mb-3">
            <button
              onClick={() => setSource("ollama")}
              className={`px-3 py-1.5 text-xs rounded-md border transition-colors ${
                source === "ollama"
                  ? "bg-brand-600/20 text-brand-300 border-brand-500"
                  : "bg-tertiary border-themed text-secondary hover:text-primary"
              }`}
            >
              Ollama Library
            </button>
            <button
              onClick={() => setSource("huggingface")}
              className={`px-3 py-1.5 text-xs rounded-md border transition-colors ${
                source === "huggingface"
                  ? "bg-brand-600/20 text-brand-300 border-brand-500"
                  : "bg-tertiary border-themed text-secondary hover:text-primary"
              }`}
            >
              HuggingFace (GGUF)
            </button>

            <div className="ml-auto flex items-center gap-2">
              {systemRam && (
                <label className="flex items-center gap-1.5 text-xs text-secondary cursor-pointer">
                  <input
                    type="checkbox"
                    checked={ramFilter}
                    onChange={e => setRamFilter(e.target.checked)}
                    className="rounded"
                  />
                  RAM filter ({systemRam.toFixed(0)} GB)
                </label>
              )}
            </div>
          </div>

          <input
            value={query}
            onChange={e => setQuery(e.target.value)}
            placeholder={source === "ollama" ? t("localllm.search_placeholder") : "Search HuggingFace for GGUF models..."}
            className="w-full rounded-lg border border-themed bg-tertiary px-3 py-2 text-sm mb-3 focus:border-brand-500 focus:outline-none"
          />

          {source === "huggingface" && !query.trim() && (
            <p className="text-xs text-secondary text-center py-4">Type a search query to find GGUF models on HuggingFace</p>
          )}

          {loading && (
            <p className="text-xs text-secondary text-center py-2">Searching...</p>
          )}

          <div className="grid grid-cols-1 gap-2 max-h-80 overflow-y-auto lg:grid-cols-2">
            {filtered.map(m => {
              const fits = fitsInRam(m);
              return (
                <div
                  key={`${m.source || "ollama"}-${m.name}`}
                  className={`flex items-start justify-between p-2 rounded-lg bg-tertiary ${
                    !fits ? "opacity-40" : ""
                  }`}
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-1.5">
                      <p className="font-mono text-xs font-medium truncate">{m.name}</p>
                      {m.ram_estimate_gb && (
                        <span className={`text-[10px] px-1.5 py-0.5 rounded-full whitespace-nowrap ${
                          fits ? "bg-emerald-500/20 text-emerald-400" : "bg-red-500/20 text-red-400"
                        }`}>
                          ~{m.ram_estimate_gb.toFixed(1)} GB
                        </span>
                      )}
                    </div>
                    <p className="text-xs text-secondary mt-0.5 line-clamp-2">{m.description}</p>
                    <div className="flex items-center gap-2 mt-1 text-[10px] text-secondary">
                      <span>Sizes: {m.sizes}</span>
                      {m.pulls && <span>· {m.pulls}</span>}
                      {m.downloads != null && <span>· {formatNumber(m.downloads)} downloads</span>}
                      {m.likes != null && m.likes > 0 && <span>· {formatNumber(m.likes)} likes</span>}
                      {m.updated && <span>· {m.updated}</span>}
                    </div>
                    {!fits && (
                      <p className="text-[10px] text-red-400 mt-0.5">Exceeds system RAM</p>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

/* ─── Installed Models ─── */

function InstalledModels() {
  const { t } = useTranslation();
  const [models, setModels] = useState<OllamaModel[]>([]);
  const [loading, setLoading] = useState(true);
  const [toggling, setToggling] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [aliasModel, setAliasModel] = useState<string | null>(null);
  const [aliasValue, setAliasValue] = useState("");
  const [preloadModels, setPreloadModels] = useState<Set<string>>(new Set());
  const [loadingModel, setLoadingModel] = useState<string | null>(null);

  const fetchModels = useCallback(async () => {
    setLoading(true);
    try {
      const data = await api.getOllamaModels() as { models: OllamaModel[] };
      setModels(data.models ?? []);
    } catch {}
    setLoading(false);
  }, []);

  // Initialize preload set from persisted server state
  useEffect(() => {
    api.ollamaPreloadList().then(data => {
      setPreloadModels(new Set(data.models ?? []));
    }).catch(() => {});
  }, []);

  useEffect(() => { fetchModels(); }, [fetchModels]);

  const toggle = async (name: string, enabled: boolean) => {
    setToggling(name);
    try {
      await api.toggleOllamaModel(name, !enabled);
      await fetchModels();
    } catch {}
    setToggling(null);
  };

  const deleteModel = async (name: string) => {
    setDeleting(name);
    try {
      await api.ollamaDeleteModel(name);
      setConfirmDelete(null);
      await fetchModels();
    } catch {}
    setDeleting(null);
  };

  const createAlias = async (model: string) => {
    if (!aliasValue.trim()) return;
    try {
      await api.ollamaAlias(model, aliasValue.trim());
      setAliasModel(null);
      setAliasValue("");
      fetchModels();
    } catch {}
  };

  const togglePreload = async (model: string) => {
    const enabled = !preloadModels.has(model);
    try {
      await api.ollamaPreload(model, enabled);
      setPreloadModels(prev => {
        const next = new Set(prev);
        if (enabled) next.add(model);
        else next.delete(model);
        return next;
      });
    } catch {}
  };

  const loadModel = async (model: string) => {
    setLoadingModel(model);
    try {
      await api.ollamaLoad(model);
    } catch {}
    setLoadingModel(null);
  };

  return (
    <div>
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-sm font-medium text-secondary">{t("localllm.installed")}</h3>
        <button onClick={fetchModels} className="btn-ghost text-xs">{t("localllm.refresh")}</button>
      </div>

      <div className="card overflow-hidden p-0">
        {loading ? (
          <div className="p-4 text-center text-secondary text-sm">Loading models...</div>
        ) : models.length === 0 ? (
          <div className="p-4 text-center text-secondary text-sm">{t("localllm.no_models")}</div>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-themed text-left text-xs text-secondary">
                <th className="px-4 py-2">{t("localllm.col_model")}</th>
                <th className="px-4 py-2">{t("localllm.col_family")}</th>
                <th className="px-4 py-2">{t("localllm.col_size")}</th>
                <th className="px-4 py-2">{t("localllm.col_params")}</th>
                <th className="px-4 py-2 text-center">{t("localllm.col_routed")}</th>
                <th className="px-4 py-2 text-center">Preload</th>
                <th className="px-4 py-2 text-right">{t("localllm.col_actions")}</th>
              </tr>
            </thead>
            <tbody>
              {models.map(m => (
                <tr key={m.name} className="border-b border-themed-faint hover:bg-hover">
                  <td className="px-4 py-2 font-mono text-xs">{m.name}</td>
                  <td className="px-4 py-2 text-secondary">{m.family || "—"}</td>
                  <td className="px-4 py-2 text-secondary">{formatSize(m.size_bytes)}</td>
                  <td className="px-4 py-2 text-secondary">{m.parameter_size || "—"}</td>
                  <td className="px-4 py-2 text-center">
                    <ToggleSwitch enabled={m.enabled} loading={toggling === m.name} onToggle={() => toggle(m.name, m.enabled)} />
                  </td>
                  <td className="px-4 py-2 text-center">
                    <ToggleSwitch enabled={preloadModels.has(m.name)} onToggle={() => togglePreload(m.name)} />
                  </td>
                  <td className="px-4 py-2 text-right">
                    <div className="flex items-center justify-end gap-2">
                      <button
                        onClick={() => loadModel(m.name)}
                        disabled={loadingModel === m.name}
                        className="text-xs text-brand-400 hover:text-brand-300 font-medium"
                        title="Load model into memory"
                      >
                        {loadingModel === m.name ? "Loading..." : "Load"}
                      </button>
                      <button
                        onClick={() => setAliasModel(aliasModel === m.name ? null : m.name)}
                        className="text-xs text-secondary hover:text-primary"
                        title="Create alias"
                      >
                        Alias
                      </button>
                      {confirmDelete === m.name ? (
                        <>
                          <button
                            onClick={() => deleteModel(m.name)}
                            disabled={deleting === m.name}
                            className="text-xs text-red-400 hover:text-red-300 font-medium"
                          >
                            {deleting === m.name ? "..." : "Confirm"}
                          </button>
                          <button onClick={() => setConfirmDelete(null)} className="text-xs text-secondary hover:text-primary">
                            Cancel
                          </button>
                        </>
                      ) : (
                        <button onClick={() => setConfirmDelete(m.name)} className="text-xs text-red-400/60 hover:text-red-400">
                          Delete
                        </button>
                      )}
                    </div>
                    {aliasModel === m.name && (
                      <div className="flex items-center gap-1 mt-1">
                        <input
                          value={aliasValue}
                          onChange={e => setAliasValue(e.target.value)}
                          placeholder="alias-name"
                          className="rounded border border-themed bg-tertiary px-2 py-0.5 text-xs w-28"
                          onKeyDown={e => e.key === "Enter" && createAlias(m.name)}
                        />
                        <button onClick={() => createAlias(m.name)} className="text-xs text-brand-400">Save</button>
                      </div>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

/* ─── Import GGUF ─── */

function ImportGGUF() {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [status, setStatus] = useState<"idle" | "importing" | "success" | "error">("idle");
  const [message, setMessage] = useState("");

  const importModel = async () => {
    if (!path.trim() || !name.trim()) return;
    setStatus("importing");
    setMessage("Importing...");
    try {
      const result = await api.ollamaImport(path.trim(), name.trim());
      if (result.status === "created") {
        setStatus("success");
        setMessage(`Model "${name}" created successfully`);
        setPath("");
        setName("");
      } else {
        setStatus("error");
        setMessage("Import failed");
      }
    } catch (e: unknown) {
      setStatus("error");
      setMessage(e instanceof Error ? e.message : "Import failed");
    }
  };

  return (
    <div>
      <div
        className="flex items-center justify-between cursor-pointer mb-3"
        onClick={() => setExpanded(!expanded)}
      >
        <h3 className="text-sm font-medium text-secondary">{t("localllm.import_gguf")}</h3>
        <span className="text-xs text-secondary">{expanded ? "▲" : "▼"}</span>
      </div>

      {expanded && (
        <div className="card space-y-3">
          <p className="text-xs text-secondary">{t("localllm.import_hint")}</p>
          <div>
            <label className="text-xs text-secondary block mb-1">{t("localllm.gguf_path")}</label>
            <input
              value={path}
              onChange={e => setPath(e.target.value)}
              placeholder="/path/to/model.gguf"
              className="w-full rounded-lg border border-themed bg-tertiary px-3 py-2 text-sm focus:border-brand-500 focus:outline-none"
            />
          </div>
          <div>
            <label className="text-xs text-secondary block mb-1">{t("localllm.model_name")}</label>
            <input
              value={name}
              onChange={e => setName(e.target.value)}
              placeholder="my-custom-model"
              className="w-full rounded-lg border border-themed bg-tertiary px-3 py-2 text-sm focus:border-brand-500 focus:outline-none"
            />
          </div>
          <button
            onClick={importModel}
            disabled={!path.trim() || !name.trim() || status === "importing"}
            className="btn-primary text-sm w-full"
          >
            {status === "importing" ? "Importing..." : t("localllm.import_btn")}
          </button>
          {message && (
            <p className={`text-xs ${status === "success" ? "text-emerald-400" : status === "error" ? "text-red-400" : "text-secondary"}`}>
              {message}
            </p>
          )}
        </div>
      )}
    </div>
  );
}

/* ─── Shared Components ─── */

function ToggleSwitch({ enabled, loading, onToggle }: { enabled: boolean; loading?: boolean; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      disabled={loading}
      className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
        enabled ? "bg-brand-500" : "bg-tertiary"
      }`}
    >
      <span className={`inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform ${
        enabled ? "translate-x-4" : "translate-x-0.5"
      }`} />
    </button>
  );
}

function formatSize(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`;
  if (bytes > 0) return `${bytes} B`;
  return "0 B";
}
