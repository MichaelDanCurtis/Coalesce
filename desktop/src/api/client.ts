import type { Stats, QuotaInfo, RequestEntry, RoutingResult, RoutingProfile, ConfigProfile, SearchParams } from "../types";

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

async function httpPut<T>(path: string, body: unknown): Promise<T> {
  const resp = await fetch(`${PROXY_BASE}${path}`, {
    method: "PUT",
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

  async refreshProvider(name: string) {
    return httpPost<{ ok: boolean; models?: number; message?: string; error?: string }>(
      `/api/v1/providers/${name}/refresh`, {}
    );
  },

  async parseDocument(data: string, filename: string): Promise<{ text: string; filename: string }> {
    return httpPost("/api/v1/parse", { data, filename });
  },

  async toggleProvider(name: string, enabled: boolean) {
    return httpPost<{ status: string; provider: string; enabled: boolean }>(
      `/api/v1/providers/${name}/toggle`, { enabled }
    );
  },

  async toggleModel(provider: string, model: string, enabled: boolean) {
    const safeModel = model.replace(/:/g, "---");
    return httpPost<{ status: string; provider: string; model: string; enabled: boolean }>(
      `/api/v1/providers/${provider}/models/${safeModel}/toggle`, { enabled }
    );
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

  // --- Profiles ---
  async getConfigProfiles() {
    return httpGet<{ profiles: ConfigProfile[]; active: string | null }>("/api/v1/profiles");
  },

  async getConfigProfile(name: string) {
    return httpGet<{ status: string; profile?: any; error?: string }>(`/api/v1/profiles/${encodeURIComponent(name)}`);
  },

  async saveProfile(name: string, description?: string) {
    return httpPost<{ status: string; name?: string; error?: string }>("/api/v1/profiles", { name, description });
  },

  async updateProfile(name: string, data: { name?: string; description?: string }) {
    const resp = await fetch(`${PROXY_BASE}/api/v1/profiles/${encodeURIComponent(name)}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(data),
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },

  async deleteProfile(name: string) {
    const resp = await fetch(`${PROXY_BASE}/api/v1/profiles/${encodeURIComponent(name)}`, { method: "DELETE" });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },

  async activateProfile(name: string) {
    return httpPost<{ status: string; providers_applied?: number; message?: string; error?: string }>(
      `/api/v1/profiles/${encodeURIComponent(name)}/activate`, {}
    );
  },

  async importProfile(profile: any) {
    return httpPost<{ status: string; name?: string; error?: string }>("/api/v1/profiles/import", profile);
  },

  async exportProfile(name: string) {
    return httpGet<{ status: string; profile?: any; error?: string }>(`/api/v1/profiles/${encodeURIComponent(name)}`);
  },

  // --- Search & Export ---
  async searchRequests(params: SearchParams) {
    const qs = new URLSearchParams();
    if (params.limit) qs.set("limit", String(params.limit));
    if (params.offset) qs.set("offset", String(params.offset));
    if (params.provider) qs.set("provider", params.provider);
    if (params.tier) qs.set("tier", params.tier);
    if (params.model) qs.set("model", params.model);
    if (params.from) qs.set("from", String(params.from));
    if (params.to) qs.set("to", String(params.to));
    if (params.search) qs.set("search", params.search);
    if (params.failures_only) qs.set("failures_only", "true");
    return httpGet<{ entries: RequestEntry[]; total: number; limit: number; offset: number; count: number }>(
      `/api/v1/stats/search?${qs.toString()}`
    );
  },

  exportJsonUrl(params?: SearchParams) {
    const qs = new URLSearchParams();
    if (params?.provider) qs.set("provider", params.provider);
    if (params?.tier) qs.set("tier", params.tier);
    if (params?.model) qs.set("model", params.model);
    if (params?.from) qs.set("from", String(params.from));
    if (params?.to) qs.set("to", String(params.to));
    if (params?.search) qs.set("search", params.search);
    if (params?.failures_only) qs.set("failures_only", "true");
    return `${PROXY_BASE}/api/v1/stats/export/json?${qs.toString()}`;
  },

  exportCsvUrl(params?: SearchParams) {
    const qs = new URLSearchParams();
    if (params?.provider) qs.set("provider", params.provider);
    if (params?.tier) qs.set("tier", params.tier);
    if (params?.model) qs.set("model", params.model);
    if (params?.from) qs.set("from", String(params.from));
    if (params?.to) qs.set("to", String(params.to));
    if (params?.search) qs.set("search", params.search);
    if (params?.failures_only) qs.set("failures_only", "true");
    return `${PROXY_BASE}/api/v1/stats/export/csv?${qs.toString()}`;
  },

  exportCostsCsvUrl(days?: number) {
    return `${PROXY_BASE}/api/v1/stats/export/costs/csv?days=${days ?? 30}`;
  },

  // --- Quality Feedback ---
  async submitFeedback(provider: string, model: string, rating: number) {
    return httpPost<{ status: string; current_score: number }>("/api/v1/feedback", { provider, model, rating });
  },

  async getQualityScores() {
    return httpGet<{ scores: Array<{ key: string; score: number; sample_count: number }> }>("/api/v1/quality/scores");
  },

  // --- Failover Rules ---
  async getRules() {
    return httpGet<{ rules: Array<any> }>("/api/v1/rules");
  },
  async createRule(rule: any) {
    return httpPost<{ status: string }>("/api/v1/rules", rule);
  },
  async getRulePresets() {
    return httpGet<{ presets: Array<any> }>("/api/v1/rules/presets");
  },
  async deleteRule(id: string) {
    const resp = await fetch(`${PROXY_BASE}/api/v1/rules/${id}`, { method: "DELETE" });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },
  async toggleRule(id: string) {
    return httpPost<{ status: string; enabled: boolean }>(`/api/v1/rules/${id}/toggle`, {});
  },

  // --- Harnesses ---
  async getHarnesses() {
    if (isTauri()) return tauriInvoke("get_harnesses");
    return httpGet<{ harnesses: Array<{
      id: string; name: string; icon: string; installed: boolean;
      configured: boolean; config_path: string | null;
      backup_exists: boolean; description: string;
    }> }>("/api/v1/harnesses");
  },

  async configureHarness(id: string) {
    if (isTauri()) return tauriInvoke("configure_harness", { id });
    return httpPost<{ success: boolean; message: string; harness_id: string }>(
      `/api/v1/harnesses/${id}/configure`, {}
    );
  },

  async restoreHarness(id: string) {
    if (isTauri()) return tauriInvoke("restore_harness", { id });
    return httpPost<{ success: boolean; message: string; harness_id: string }>(
      `/api/v1/harnesses/${id}/restore`, {}
    );
  },

  // --- Model Overrides ---
  async getOverrides() {
    return httpGet<{ overrides: Array<{ provider: string; model_id: string; field: string; value: string }> }>("/api/v1/overrides");
  },

  async getModelOverrides(provider: string, modelId: string) {
    return httpGet<{ provider: string; model_id: string; overrides: Record<string, string> }>(
      `/api/v1/overrides/${encodeURIComponent(provider)}/${encodeURIComponent(modelId)}`
    );
  },

  async setModelOverrides(provider: string, modelId: string, overrides: Record<string, string>) {
    const resp = await fetch(
      `${PROXY_BASE}/api/v1/overrides/${encodeURIComponent(provider)}/${encodeURIComponent(modelId)}`,
      { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ overrides }) }
    );
    return resp.json();
  },

  async clearModelOverrides(provider: string, modelId: string) {
    const resp = await fetch(
      `${PROXY_BASE}/api/v1/overrides/${encodeURIComponent(provider)}/${encodeURIComponent(modelId)}`,
      { method: "DELETE" }
    );
    return resp.json();
  },

  // --- Cache ---
  async getCacheStats() {
    return httpGet<{ enabled: boolean; entries: number; max_entries: number; ttl_secs: number; total_hits: number }>("/api/v1/cache/stats");
  },

  async clearCache() {
    return httpPost<{ cleared: boolean }>("/api/v1/cache/clear", {});
  },

  async toggleCache(enabled: boolean) {
    return httpPut<{ enabled: boolean; entries: number; max_entries: number; ttl_secs: number; total_hits: number }>("/api/v1/cache/config", { enabled });
  },

  // --- Mock Provider ---
  async getMockStatus() {
    return httpGet<{ enabled: boolean }>("/api/v1/mock/status");
  },

  async toggleMock() {
    return httpPost<{ enabled: boolean }>("/api/v1/mock/toggle", {});
  },

  // --- Thinking Optimizer ---
  async getThinkingStatus() {
    return httpGet<{ enabled: boolean; min_complexity: number; max_budget_tokens: number; default_budget_tokens: number }>("/api/v1/thinking/status");
  },

  async toggleThinking(enabled: boolean) {
    return httpPut<{ enabled: boolean; min_complexity: number; max_budget_tokens: number; default_budget_tokens: number }>("/api/v1/thinking/config", { enabled });
  },

  // --- Tokens ---
  async getTokens() {
    return httpGet<{ tokens: Array<{ provider: string; type: string; valid: boolean }>; count: number }>("/api/v1/tokens");
  },

  async getExpiringTokens() {
    return httpGet<{ expiring_soon: string[] }>("/api/v1/tokens/expiring");
  },

  // --- Harness Takeover ---
  async takeoverAllHarnesses() {
    return httpPost<{ results: Array<{ success: boolean; message: string; harness_id: string }> }>("/api/v1/harnesses/takeover", {});
  },

  async restoreAllHarnesses() {
    return httpPost<{ results: Array<{ success: boolean; message: string; harness_id: string }> }>("/api/v1/harnesses/restore-all", {});
  },

  // --- MCP Servers ---
  async getMcpServers() {
    return httpGet<{ servers: Array<{
      id: string; name: string; transport: string; status: string;
      tools: Array<{ name: string; description?: string }>;
      enabled: boolean; source: string; command?: string; url?: string;
      last_connected?: number; error?: string;
    }> }>("/api/v1/mcp/servers");
  },

  async scanMcpServers() {
    return httpPost<{ servers: Array<any> }>("/api/v1/mcp/scan", {});
  },

  async toggleMcpServer(id: string) {
    return httpPost<{ enabled: boolean }>(`/api/v1/mcp/servers/${id}/toggle`, {});
  },

  async registerMcpServer(server: { name: string; transport: string; command?: string; args?: string[]; url?: string }) {
    return httpPost<{ ok: boolean; id: string }>("/api/v1/mcp/servers", server);
  },

  async removeMcpServer(id: string) {
    const resp = await fetch(`${PROXY_BASE}/api/v1/mcp/servers/${id}`, { method: "DELETE" });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  },
};
