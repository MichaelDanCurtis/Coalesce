import type { Stats, QuotaInfo, RequestEntry, RoutingResult, RoutingProfile } from "../types";

// Detect if running inside Tauri
const isTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// Dynamic import for Tauri invoke
async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

const PROXY_BASE = "http://127.0.0.1:8402";

async function httpGet<T>(path: string): Promise<T> {
  const resp = await fetch(`${PROXY_BASE}${path}`);
  if (!resp.ok) throw new Error(`HTTP ${resp.status}: ${resp.statusText}`);
  return resp.json();
}

async function httpPost<T>(path: string, body: unknown): Promise<T> {
  const resp = await fetch(`${PROXY_BASE}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!resp.ok) throw new Error(`HTTP ${resp.status}: ${resp.statusText}`);
  return resp.json();
}

export const api = {
  async getProviders() {
    if (isTauri()) return tauriInvoke("get_providers");
    return httpGet("/api/v1/providers");
  },

  async getStats(): Promise<{ stats: Stats; quotas: QuotaInfo[]; recent_requests: RequestEntry[] }> {
    if (isTauri()) return tauriInvoke("get_stats");
    return httpGet("/api/v1/stats/summary");
  },

  async getModels() {
    if (isTauri()) return tauriInvoke("get_models");
    return httpGet("/v1/models");
  },

  async getHealth() {
    if (isTauri()) return tauriInvoke("get_health");
    return httpGet("/health");
  },

  async routingPlayground(prompt: string, weights?: Record<string, number>): Promise<RoutingResult> {
    const body = weights ? { prompt, weights } : { prompt };
    if (isTauri()) return tauriInvoke("routing_playground", body);
    return httpPost("/api/v1/routing/playground", body);
  },

  async getQuotas(): Promise<{ quotas: QuotaInfo[] }> {
    if (isTauri()) return tauriInvoke("get_quotas");
    return httpGet("/api/v1/providers/quotas");
  },

  async getProfiles(): Promise<{ profiles: RoutingProfile[] }> {
    if (isTauri()) return tauriInvoke("get_profiles");
    return httpGet("/api/v1/routing/profiles");
  },

  async getRoutingPins() {
    return httpGet<{ pins: Record<string, Array<{ model_id: string; providers: string[] }>> }>("/api/v1/routing/pins");
  },

  async setRoutingPins(pins: Record<string, Array<{ model_id: string; providers: string[] }>>) {
    const resp = await fetch(`${PROXY_BASE}/api/v1/routing/pins`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ pins }),
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },

  async getModelEquivalences() {
    return httpGet<{ equivalences: Record<string, string[]> }>("/api/v1/routing/equivalences");
  },

  async setModelEquivalences(equivalences: Record<string, string[]>) {
    const resp = await fetch(`${PROXY_BASE}/api/v1/routing/equivalences`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ equivalences }),
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },

  async getProviderPriorities() {
    return httpGet<{ providers: Record<string, { priority: number; pricing_mode: string }> }>("/api/v1/providers/priorities");
  },

  async setProviderPriorities(providers: Record<string, { priority: number; pricing_mode: string }>) {
    const resp = await fetch(`${PROXY_BASE}/api/v1/providers/priorities`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ providers }),
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },

  async getTimeline(limit = 20, offset = 0): Promise<{ requests: RequestEntry[] }> {
    if (isTauri()) return tauriInvoke("get_timeline", { limit, offset });
    return httpGet(`/api/v1/stats/timeline?limit=${limit}&offset=${offset}`);
  },

  async getCosts(days = 30) {
    if (isTauri()) return tauriInvoke("get_costs", { days });
    return httpGet(`/api/v1/stats/costs?days=${days}`);
  },

  async createProvider(name: string, config: Record<string, unknown>) {
    return httpPost("/api/v1/providers/manage", { name, ...config });
  },

  async updateProvider(name: string, config: Record<string, any>) {
    const resp = await fetch(`${PROXY_BASE}/api/v1/providers/manage/${name}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(config),
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },

  async deleteProvider(name: string) {
    const resp = await fetch(`${PROXY_BASE}/api/v1/providers/manage/${name}`, { method: "DELETE" });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },

  async testProvider(name: string) {
    return httpPost(`/api/v1/providers/${name}/test`, {});
  },

  async copilotAuthStart() {
    return httpPost<{ user_code: string; verification_uri: string; device_code: string; interval: number }>("/api/v1/auth/copilot/start", {});
  },

  async copilotAuthPoll(device_code: string, provider_name?: string) {
    return httpPost<{ status: string; models?: number; error?: string; raw?: string }>("/api/v1/auth/copilot/poll", { device_code, provider_name });
  },

  async getOllamaModels() {
    return httpGet<{ models: Array<{ name: string; size_bytes: number; parameter_size: string; family: string; enabled: boolean }> }>("/api/v1/providers/ollama/models");
  },

  async toggleOllamaModel(model: string, enabled: boolean) {
    const safeName = model.replace(":", "---");
    return httpPost(`/api/v1/providers/ollama/models/${safeName}/toggle`, { enabled });
  },

  async ollamaPull(model: string): Promise<Response> {
    return fetch(`${PROXY_BASE}/api/v1/ollama/pull`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ model }),
    });
  },

  async ollamaDeleteModel(model: string) {
    const safeName = model.replace(":", "---");
    const resp = await fetch(`${PROXY_BASE}/api/v1/ollama/models/${safeName}`, { method: "DELETE" });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },

  async ollamaRunning() {
    return httpGet<{ models: Array<{
      name: string; size_bytes: number; size_vram: number;
      gpu_percent: number; processor: string; parameter_size: string;
      family: string; quantization: string; expires_at: string;
    }> }>("/api/v1/ollama/running");
  },

  async ollamaStart() {
    return httpPost<{ status: string; error?: string }>("/api/v1/ollama/start", {});
  },

  async ollamaStop() {
    return httpPost<{ status: string; error?: string }>("/api/v1/ollama/stop", {});
  },

  async ollamaStatus() {
    return httpGet<{ running: boolean; version: string; gpu: { gpus: Array<Record<string, unknown>>; acceleration: string } }>("/api/v1/ollama/status");
  },

  async ollamaLibrarySearch(q?: string) {
    const qs = q ? `?q=${encodeURIComponent(q)}` : "";
    return httpGet<{ models: Array<{ name: string; label: string; sizes: string; description: string }> }>(`/api/v1/ollama/library/search${qs}`);
  },

  async ollamaLibraryTags(model: string) {
    return httpGet<{ model: string; tags: unknown[]; source?: string }>(`/api/v1/ollama/library/${model}/tags`);
  },

  async ollamaKeepAlive(model: string, duration: string) {
    const safeName = model.replace(":", "---");
    return httpPost<{ status: string }>(`/api/v1/ollama/models/${safeName}/keepalive`, { duration });
  },

  async ollamaBenchmark(model: string) {
    const safeName = model.replace(":", "---");
    return httpPost<{
      model: string; tokens_generated: number; generation_tokens_per_sec: number;
      prompt_tokens: number; prompt_tokens_per_sec: number; total_duration_ms: number;
    }>(`/api/v1/ollama/models/${safeName}/benchmark`, {});
  },

  async ollamaAlias(model: string, alias: string) {
    const safeName = model.replace(":", "---");
    return httpPost<{ status: string; alias: string }>(`/api/v1/ollama/models/${safeName}/alias`, { alias });
  },

  async ollamaPreload(model: string, enabled: boolean) {
    const safeName = model.replace(":", "---");
    return httpPost<{ status: string }>(`/api/v1/ollama/models/${safeName}/preload`, { enabled });
  },

  async ollamaLoad(model: string) {
    const safeName = model.replace(":", "---");
    return httpPost<{ status: string; model: string }>(`/api/v1/ollama/models/${safeName}/load`, {});
  },

  async ollamaPreloadList() {
    return httpGet<{ models: string[] }>("/api/v1/ollama/preload");
  },

  async ollamaGpuLayers(model: string, num_gpu: number) {
    const safeName = model.replace(":", "---");
    return httpPost<{ status: string }>(`/api/v1/ollama/models/${safeName}/gpu-layers`, { num_gpu });
  },

  async ollamaImport(path: string, name: string) {
    return httpPost<{ status: string; name: string }>("/api/v1/ollama/import", { path, name });
  },

  async ollamaResources() {
    return httpGet<{
      models: Array<{ name: string; ram_bytes: number; vram_bytes: number }>;
      total_model_ram: number; total_model_vram: number;
      system: { total_bytes?: number; used_bytes?: number; free_bytes?: number };
    }>("/api/v1/ollama/resources");
  },

  async googleAuthStart() {
    return httpPost<{ auth_url: string; status: string; error?: string }>(
      "/api/v1/auth/google/start", {}
    );
  },

  async googleAuthPoll(device_code: string) {
    return httpPost<{ status: string; models?: number; error?: string }>(
      "/api/v1/auth/google/poll", { device_code }
    );
  },
};
